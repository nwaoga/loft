"""Watch indexed folders for creates, edits, deletes, and moves.

A change marks the folder stale. Reindexing stays on Refresh.
Hidden paths, build folders, and non-text files are ignored so a
dependency install does not look like an edit to the library.
"""
import os
from pathlib import Path
import threading

from loft_engine import CODE, DOCUMENTS, PDF, SKIP_DIRS


def watch_should_mark(root, path, is_directory=False):
    """Whether a filesystem event under root should mark that folder stale."""
    root = Path(root)
    path = Path(path)
    try:
        relative = path.resolve().relative_to(root.resolve())
    except (OSError, ValueError):
        try:
            relative = path.absolute().relative_to(root.absolute())
        except ValueError:
            return False
    if not relative.parts:
        return False
    for part in relative.parts[:-1]:
        if part.startswith(".") or part.lower() in SKIP_DIRS:
            return False
    name = relative.parts[-1]
    if name.startswith(".") or name.lower() in SKIP_DIRS:
        return False
    if is_directory:
        return True
    return path.suffix.lower() in DOCUMENTS | CODE | PDF


class _Handler:
    def __init__(self, roots, on_change):
        self.roots = [str(root) for root in roots]
        self.on_change = on_change

    def root_for(self, raw):
        path = Path(raw)
        try:
            path = path.resolve()
        except OSError:
            path = path.absolute()
        match = None
        for root in self.roots:
            root_path = Path(root)
            if path == root_path or root_path in path.parents or str(path).startswith(str(root_path) + os.sep):
                if match is None or len(root) > len(match):
                    match = root
        return match

    def consider(self, raw, is_directory):
        root = self.root_for(raw)
        if root and watch_should_mark(root, raw, is_directory):
            self.on_change(root)


class FolderWatcher:
    """watchdog observer. close() stops it. set_roots() replaces the set.

    Rapid writes for one folder collapse into a single stale notice after
    `delay` seconds of quiet, so typing does not schedule an embed per keystroke.
    """

    def __init__(self, on_change, delay=0.4):
        self.on_change = on_change
        self.delay = delay
        self._observer = None
        self._roots = []
        self._lock = threading.Lock()
        self._pending = set()
        self._timer = None

    def set_roots(self, roots):
        roots = sorted({str(root) for root in roots})
        with self._lock:
            if roots == self._roots and self._observer is not None:
                return
            self._roots = roots
            self._stop_locked()
            if not roots:
                return
            try:
                from watchdog.events import FileSystemEventHandler
                from watchdog.observers import Observer
            except ImportError:
                return
            handler = self._event_handler(FileSystemEventHandler)
            observer = Observer()
            for root in roots:
                if Path(root).is_dir():
                    observer.schedule(handler, root, recursive=True)
            observer.daemon = True
            observer.start()
            self._observer = observer

    def _event_handler(self, base):
        outer = self
        class Handler(base):
            def on_created(self, event):
                outer._consider(event.src_path, event.is_directory)
            def on_modified(self, event):
                if not event.is_directory:
                    outer._consider(event.src_path, False)
            def on_deleted(self, event):
                outer._consider(event.src_path, event.is_directory)
            def on_moved(self, event):
                outer._consider(event.src_path, event.is_directory)
                outer._consider(event.dest_path, event.is_directory)
        return Handler()

    def _consider(self, raw, is_directory):
        with self._lock:
            roots = list(self._roots)
        logic = _Handler(roots, self._queue)
        logic.consider(raw, is_directory)

    def notify(self, path, is_directory=False):
        """Record one create, modify, or delete without starting a reindex."""
        self._consider(path, is_directory)

    def _queue(self, root):
        with self._lock:
            self._pending.add(root)
            if self._timer is not None:
                self._timer.cancel()
            self._timer = threading.Timer(self.delay, self._flush)
            self._timer.daemon = True
            self._timer.start()

    def _flush(self):
        with self._lock:
            pending = list(self._pending)
            self._pending.clear()
            self._timer = None
        for root in pending:
            self.on_change(root)

    def close(self):
        with self._lock:
            if self._timer is not None:
                self._timer.cancel()
                self._timer = None
            self._pending.clear()
            self._stop_locked()

    def _stop_locked(self):
        observer = self._observer
        self._observer = None
        if observer is not None:
            observer.stop()
            observer.join(timeout=2)
