"""Interactive, offline notch preview. Run with python loft_preview.py."""
import argparse
import json
import re
import sqlite3
import sys
import time
import queue
import threading
import subprocess
import traceback
from pathlib import Path
import tkinter as tk
from tkinter import scrolledtext, filedialog, messagebox

from loft_bench import ROOT, FakeEmbedding, Index
from loft_engine import format_dropped
from loft_theme import windows_palette

BG = "#101113"
PANEL = "#1c1e22"
TEXT = "#f1f2f4"
MUTED = "#a0a5af"
ACCENT = "#b8d8ca"
HOVER = "#30343a"
ERROR = "#f0b4a8"
STOP = set("a an the i my have has is are was were for to of in on does do where when which files involved taking today me find paid".split())
CODE_WORDS = set("def class return if elif else for while import from as try except with lambda pass break continue and or not in True False None python powershell pwsh".split())


def monitors():
    """Work areas of attached screens. The primary screen is first."""
    if sys.platform != "win32":
        return [{"primary": True, "left": 0, "top": 0, "right": 1920, "bottom": 1080}]
    import ctypes
    class RECT(ctypes.Structure):
        _fields_ = [("left", ctypes.c_long), ("top", ctypes.c_long), ("right", ctypes.c_long), ("bottom", ctypes.c_long)]
    class MONITORINFO(ctypes.Structure):
        _fields_ = [("cbSize", ctypes.c_ulong), ("rcMonitor", RECT), ("rcWork", RECT), ("dwFlags", ctypes.c_ulong)]
    found = []
    def collect(monitor, hdc, rect, data):
        info = MONITORINFO()
        info.cbSize = ctypes.sizeof(MONITORINFO)
        ctypes.windll.user32.GetMonitorInfoW(monitor, ctypes.byref(info))
        area = info.rcWork
        found.append({"primary": bool(info.dwFlags & 1), "left": area.left, "top": area.top,
                      "right": area.right, "bottom": area.bottom})
        return 1
    callback = ctypes.WINFUNCTYPE(ctypes.c_int, ctypes.c_void_p, ctypes.c_void_p, ctypes.POINTER(RECT), ctypes.c_ssize_t)
    ctypes.windll.user32.EnumDisplayMonitors(0, 0, callback(collect), 0)
    return sorted(found, key=lambda screen: (not screen["primary"], screen["left"], screen["top"])) or [
        {"primary": True, "left": 0, "top": 0, "right": 1920, "bottom": 1080}]


def virtual_bounds(screens):
    return (min(s["left"] for s in screens), min(s["top"] for s in screens),
            max(s["right"] for s in screens), max(s["bottom"] for s in screens))


def window_handle(widget):
    import ctypes
    child = widget.winfo_id()
    parent = ctypes.windll.user32.GetParent(child)
    return parent or child


def apply_window_material(widget, width, height, glass, opacity):
    """Rounded window. Glass tints the wallpaper through the panel on Windows."""
    if sys.platform != "win32" or width < 2 or height < 2:
        return
    import ctypes
    widget.attributes("-alpha", opacity if glass else 1)
    hwnd = window_handle(widget)
    class ACCENTPOLICY(ctypes.Structure):
        _fields_ = [("AccentState", ctypes.c_uint), ("AccentFlags", ctypes.c_uint),
                    ("GradientColor", ctypes.c_uint), ("AnimationId", ctypes.c_uint)]
    class WINCOMPATTRDATA(ctypes.Structure):
        _fields_ = [("Attribute", ctypes.c_int), ("Data", ctypes.POINTER(ACCENTPOLICY)),
                    ("SizeOfData", ctypes.c_size_t)]
    accent = ACCENTPOLICY()
    accent.AccentState = 4 if glass else 0
    accent.AccentFlags = 2
    accent.GradientColor = (170 << 24) | (0x14 << 16) | (0x16 << 8) | 0x12
    data = WINCOMPATTRDATA()
    data.Attribute = 19
    data.SizeOfData = ctypes.sizeof(accent)
    data.Data = ctypes.pointer(accent)
    ctypes.windll.user32.SetWindowCompositionAttribute(hwnd, ctypes.byref(data))
    backdrop = ctypes.c_int(3 if glass else 1)
    ctypes.windll.dwmapi.DwmSetWindowAttribute(hwnd, 38, ctypes.byref(backdrop), ctypes.sizeof(backdrop))
    radius = 18 if height < 64 else 28
    region = ctypes.windll.gdi32.CreateRoundRectRgn(0, 0, int(width) + 1, int(height) + 1, radius * 2, radius * 2)
    ctypes.windll.user32.SetWindowRgn(hwnd, region, True)


def motion_enabled():
    if sys.platform != "win32":
        return False  # Until native reduced-motion preferences are supported.
    import ctypes
    from ctypes import wintypes
    enabled = wintypes.BOOL()
    return bool(ctypes.windll.user32.SystemParametersInfoW(
        0x1042, 0, ctypes.byref(enabled), 0) and enabled.value)


class DemoSearch:
    def __init__(self):
        passages = json.loads((ROOT / "fixtures/passages.json").read_text(encoding="utf-8"))
        self.indexes = {}
        for scope in ("All", "Documents", "Code"):
            index = Index(":memory:", FakeEmbedding())
            index.sync([p for p in passages if scope == "All" or
                        (p["path"].startswith("src/")) == (scope == "Code")])
            self.indexes[scope] = index

    def search(self, question, scope):
        terms = set(re.findall(r"\w+", question.lower())) - STOP
        if not terms:
            return []
        # Hash vectors are synthetic: require lexical evidence before displaying a hit.
        hits = self.indexes[scope].search(question, limit=5)
        hits = [h for h in hits if terms.intersection(re.findall(r"\w+", (h["text"] + " " + h["path"]).lower()))]
        for i, hit in enumerate(hits, 1):
            hit["citation"] = f"S{i}"
        return hits[:3]

    def close(self):
        for index in self.indexes.values():
            index.db.close()


class RoundedSurface(tk.Canvas):
    """Rounded input surround with a normal accessible Entry inside."""
    def __init__(self, parent):
        super().__init__(parent, bg=BG, height=44, highlightthickness=0)
        self.inner = tk.Frame(self, bg=PANEL)
        self.window_id = self.create_window(12, 7, window=self.inner, anchor="nw")
        self.bind("<Configure>", self.draw)

    def draw(self, event=None):
        width = self.winfo_width()
        self.delete("surface")
        radius, height = 14, 44
        self.create_rectangle(radius, 0, width-radius, height, fill=PANEL, outline="", tags="surface")
        self.create_rectangle(0, radius, width, height-radius, fill=PANEL, outline="", tags="surface")
        for x, y, start in ((0, 0, 90), (width-2*radius, 0, 0),
                            (0, height-2*radius, 180), (width-2*radius, height-2*radius, 270)):
            self.create_arc(x, y, x+2*radius, y+2*radius, start=start, extent=90,
                            fill=PANEL, outline="", tags="surface")
        self.tag_lower("surface")
        self.itemconfigure(self.window_id, width=max(1, width-24), height=30)


class LoftPreview:
    def __init__(self, root, demo=False, use_settings=True):
        self.root = root
        self.demo = demo
        self.searcher = DemoSearch() if demo else None
        self.engine = None
        self.busy = False
        self.busy_started = None
        self.phase_text = ""
        self.job_kind = None
        self._shown_second = None
        self._shown_phase = None
        self.closed = False
        self.events = queue.Queue()
        self.cancel_work = threading.Event()
        self.worker = None
        self.library_window = None
        self.answer_window = None
        if not demo:
            from loft_engine import Engine
            self.engine = Engine()
        self.expanded = False
        self.animation_timer = None
        self.motion_ready = False
        self.scope = "All"
        self.source_window = None
        self.last_hits = []
        self.answer_card = None
        self.answer_text = None
        self.answer_buffer = ""
        self.stop_button = None
        self.sources_detail = None
        self.sources_toggle = None
        self.sources_open = False
        self.support_gaps = []
        self.support_note = None
        self.chat_history = []
        self.pending_chat = ""
        self._stale_checked = 0.0
        self.library_topmost = False
        self.pending_folders = []
        self.folder_rows = []
        self._status_error = False
        self.awaiting_empty_note = False
        self.origin = None
        self._press = None
        self.use_settings = use_settings
        self.prefs = self.load_prefs() if use_settings else {}
        self.glass = bool(self.prefs.get("glass", True)) if use_settings else False
        self.opacity = float(self.prefs.get("opacity", 0.78))
        self.topmost = bool(self.prefs.get("topmost", True))
        if use_settings and "x" in self.prefs and "y" in self.prefs:
            self.origin = (int(self.prefs["x"]), int(self.prefs["y"]))
        root.title("Loft — desktop preview")
        root.configure(bg=BG)
        root.overrideredirect(True)
        root.attributes("-topmost", self.topmost)
        root.protocol("WM_DELETE_WINDOW", self.close)
        root.bind("<Escape>", self.on_escape)
        root.bind("<Control-q>", lambda _: self.close())
        root.bind("<Control-k>", lambda _: self.focus_search())
        root.bind("<Command-k>", lambda _: self.focus_search()) if sys.platform == "darwin" else None
        self.canvas = tk.Canvas(root, bg=BG, highlightthickness=0)
        self.canvas.pack(fill="both", expand=True)
        self.body = tk.Frame(self.canvas, bg=BG)
        self.body_id = self.canvas.create_window(24, 18, window=self.body, anchor="nw")
        self.compact = tk.Frame(self.canvas, bg=BG)
        self.compact_id = self.canvas.create_window(18, 5, window=self.compact, anchor="nw")
        self.compact_title = tk.Label(self.compact, text="loft    Ask your files    Ctrl+K", bg=BG, fg=TEXT,
                                      font=("Segoe UI", 10), cursor="fleur")
        self.compact_title.pack(side="left", padx=(4, 0))
        self.bind_drag(self.compact_title, self.focus_search)
        self.compact_stop = self.button(self.compact, "Stop", lambda: self.cancel_work.set(), muted=True)
        self.compact_close = self.button(self.compact, "×", self.close, muted=True)
        self.compact_close.pack(side="left")
        header = self.header = tk.Frame(self.body, bg=BG, cursor="fleur")
        header.pack(fill="x")
        self.bind_drag(header)
        title = tk.Label(header, text="loft", bg=BG, fg=TEXT, font=("Segoe UI", 13, "bold"), cursor="fleur")
        title.pack(side="left")
        self.bind_drag(title)
        local = tk.Label(header, text="  ·  Local", bg=BG, fg=ACCENT, font=("Segoe UI", 9), cursor="fleur")
        local.pack(side="left")
        self.bind_drag(local)
        self.button(header, "×", self.close, muted=True).pack(side="right")
        self.button(header, "−", self.collapse, muted=True).pack(side="right")
        tk.Label(header, text="Ctrl+K", bg=BG, fg=MUTED, font=("Segoe UI", 9)).pack(side="right", padx=(0, 6))
        self.input_surface = RoundedSurface(self.body)
        self.input_surface.pack(fill="x", pady=(8, 6))
        row = self.input_surface.inner
        tk.Label(row, text="\u2315", bg=PANEL, fg=MUTED, font=("Segoe UI Symbol", 12)).pack(side="left", padx=(2, 4))
        self.question = tk.StringVar()
        self.entry = tk.Entry(row, textvariable=self.question, bg=PANEL, fg=TEXT,
                              insertbackground=ACCENT, relief="flat", font=("Segoe UI", 13))
        self.entry.pack(side="left", fill="x", expand=True, ipady=2)
        self.entry.bind("<Return>", lambda _: self.search())
        self.button(row, "↵", self.search).pack(side="right")
        self.placeholder = tk.Label(self.entry, text="Ask your files…", bg=PANEL,
                                    fg=MUTED, font=("Segoe UI", 12), cursor="xterm")
        self.placeholder.place(x=0, rely=0.5, anchor="w")
        self.placeholder.bind("<Button-1>", lambda _: self.entry.focus_set())
        self.question.trace_add("write", self.update_placeholder)
        scopes = self.scopes = tk.Frame(self.body, bg=BG)
        scopes.pack(fill="x", pady=(0, 6))
        self.scope_buttons = {}
        for scope in (("All", "Documents", "Code") if demo else ("All", "Documents", "Code", "Chat")):
            button = self.button(scopes, "Everything" if scope == "All" else scope, lambda s=scope: self.select_scope(s), muted=True)
            button.pack(side="left", padx=(12 if scope == "Chat" else 0, 4))
            self.scope_buttons[scope] = button
        if not demo:
            self.button(scopes, "Folders", self.manage_library, muted=True).pack(side="left", padx=(8, 0))
        self.button(scopes, "Settings", self.show_settings, muted=True).pack(side="left", padx=(8, 0))
        self.status = tk.Label(self.body, bg=BG, fg=MUTED, anchor="w", justify="left",
                               font=("Segoe UI", 10), wraplength=490)
        self.status.pack(fill="x", pady=(0, 4))
        self.scroll_host = tk.Frame(self.body, bg=BG)
        self.scroll_host.pack(fill="x")
        self.scroll = tk.Canvas(self.scroll_host, bg=BG, height=1, highlightthickness=0, bd=0)
        self.vbar = tk.Scrollbar(self.scroll_host, orient="vertical", command=self.scroll.yview,
                                 bg=PANEL, troughcolor=BG, activebackground=HOVER, highlightthickness=0)
        self.scroll.configure(yscrollcommand=self._on_results_scroll)
        self.scroll.pack(side="left", fill="x", expand=True)
        self.scroll_inner = tk.Frame(self.scroll, bg=BG)
        self.scroll_window = self.scroll.create_window((0, 0), window=self.scroll_inner, anchor="nw")
        self.results = tk.Frame(self.scroll_inner, bg=BG)
        self.results.pack(fill="x")
        self.scroll.bind("<Configure>", self._sync_scroll_width)
        self.root.bind_all("<MouseWheel>", self._on_results_wheel)
        self.root.bind_all("<Button-4>", self._on_results_wheel)
        self.root.bind_all("<Button-5>", self._on_results_wheel)
        footer = self.footer = tk.Frame(self.body, bg=BG)
        footer.pack(fill="x", pady=(8, 0))
        self.library_label = tk.Label(footer, text="Sample library · 5 files · Search only" if demo else f"Your library · {self.engine.count()} files · On device",
                 bg=BG, fg=MUTED, font=("Segoe UI", 9))
        self.library_label.pack(side="left")
        self.drag_hint = tk.Label(footer, text="drag title to move", bg=BG, fg=MUTED, font=("Segoe UI", 9))
        self.drag_hint.pack(side="right")
        self.style_scopes()
        self.show_welcome()
        self.collapse()
        self.motion_ready = True
        self.theme_timer = None
        self.refresh_theme()
        self.event_timer = self.root.after(50, self.poll_events) if not demo else None

    def on_escape(self, _event=None):
        if self.busy and not self.demo:
            self.cancel_work.set()
            self.note_phase("Stopping…")
            return
        self.collapse()

    def start_job(self, operation, kind=None):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return False
        self.busy = True
        self.job_kind = kind
        self._status_error = False
        self.busy_started = time.perf_counter()
        self._shown_second = None
        self._shown_phase = None
        self.cancel_work = threading.Event()
        self.entry.configure(state="disabled")
        self.root.after_idle(self.fit_content)
        captured = kind
        def emit(kind, value):
            self.events.put((kind, value))
        def work():
            try:
                result = operation(self.cancel_work, emit)
                emit("done", result)
            except Exception as exc:
                emit("error", self._worker_error(exc, captured))
            finally:
                emit("idle", {"cancelled": self.cancel_work.is_set()})
        self.worker = threading.Thread(target=work, daemon=True)
        self.worker.start()
        return True

    def _worker_error(self, exc, kind):
        if self.cancel_work.is_set():
            if kind == "index":
                return "Stopped. Your saved index is unchanged."
            return "Stopped."
        text = str(exc).strip() or exc.__class__.__name__
        return text

    def show_stop(self):
        if self.stop_button is None or not self.stop_button.winfo_exists():
            self.stop_button = self.button(self.footer, "Stop", lambda: self.cancel_work.set())
            self.stop_button.configure(bg=ACCENT, fg=BG, activebackground=ACCENT, activeforeground=BG)
            self.stop_button.pack(side="right", padx=(0, 6), before=self.drag_hint)
        if self.compact_stop is not None and not self.compact_stop.winfo_ismapped():
            self.compact_stop.pack(side="left", before=self.compact_close)

    def hide_stop(self):
        if self.stop_button is not None and self.stop_button.winfo_exists():
            self.stop_button.destroy()
        self.stop_button = None
        if self.compact_stop is not None and self.compact_stop.winfo_ismapped():
            self.compact_stop.pack_forget()

    def restore_compact(self):
        self.compact_title.configure(text="loft    Ask your files    Ctrl+K")

    def note_phase(self, text):
        self.phase_text = text
        if self.busy:
            self.paint_activity(force=True)
        else:
            self.status.configure(text=text, fg=ERROR if self._status_error else MUTED)

    def paint_activity(self, force=False):
        if not self.busy or self.busy_started is None or not self.phase_text:
            return
        second = int(time.perf_counter() - self.busy_started)
        if not force and second == self._shown_second and self.phase_text == self._shown_phase:
            return
        self._shown_second = second
        self._shown_phase = self.phase_text
        self.status.configure(text=f"{self.phase_text}  {second}s", fg=ACCENT)
        short = self.phase_text if len(self.phase_text) <= 42 else self.phase_text[:41] + "…"
        self.compact_title.configure(text=f"loft    {short}    {second}s")

    def show_note(self, message, color=None):
        tk.Label(self.results, text=message, bg=BG, fg=color or TEXT, wraplength=490,
                 justify="left", anchor="w", font=("Segoe UI", 11)).pack(anchor="w", fill="x", pady=(4, 6))

    def poll_events(self):
        if self.closed:
            return
        try:
            self.drain_events()
            self.refresh_stale_markers()
        except Exception:
            traceback.print_exc()
            if self.worker is None or not self.worker.is_alive():
                self.busy = False
                self.job_kind = None
                self.hide_stop()
                try:
                    self.entry.configure(state="normal")
                except tk.TclError:
                    pass
            try:
                self.status.configure(text="Something went wrong updating the window. You can try again.", fg=ERROR)
            except tk.TclError:
                pass
        finally:
            if not self.closed:
                try:
                    self.event_timer = self.root.after(50, self.poll_events)
                except tk.TclError:
                    pass

    def drain_events(self):
        changed = False
        for _ in range(100):
            try:
                kind, value = self.events.get_nowait()
            except queue.Empty:
                break
            if kind == "status":
                self._status_error = False
                self.note_phase(value)
                if self.job_kind == "index":
                    self.set_library_status(value, ACCENT)
                if self.awaiting_empty_note:
                    self.show_note(value)
                    self.awaiting_empty_note = False
                changed = True
            elif kind == "chat":
                self.awaiting_empty_note = False
                if self.pending_chat:
                    self.chat_history.append({"role": "user", "content": self.pending_chat})
                    self.pending_chat = ""
                self.chat_history.append({"role": "assistant", "content": value})
                self.chat_history = self.chat_history[-6:]
                self.clear_results()
                self.render_chat_transcript()
                self.root.lift()
                changed = True
            elif kind == "support":
                self.support_gaps = list(value or [])
                self.show_support_flag()
                changed = True
            elif kind == "sources":
                self.support_gaps = []
                self.last_hits = value or []
                self.clear_results()
                if value:
                    self.awaiting_empty_note = False
                    self.present_streaming()
                    self.present_sources(value)
                    actions = tk.Frame(self.results, bg=BG)
                    actions.pack(fill="x", pady=(6, 0))
                    self.button(actions, "Read full answer", self.show_answer, muted=True).pack(side="left")
                else:
                    self.awaiting_empty_note = True
                self.root.lift()
                changed = True
            elif kind == "token":
                self.answer_buffer += value
                preview = self.answer_buffer[:450]
                if self.answer_text is not None and self.answer_text.winfo_exists():
                    shown = preview + ("…" if len(self.answer_buffer) > 450 else "")
                    self.answer_text.configure(text=shown)
                if self.answer_window and self.answer_window.winfo_exists() and getattr(self, "full_answer", None):
                    self.full_answer.configure(state="normal")
                    self.full_answer.insert("end", value)
                    self.full_answer.see("end")
                    self.full_answer.configure(state="disabled")
                changed = True
            elif kind == "warning":
                tk.Label(self.results, text=value, bg=BG, fg=ACCENT, wraplength=490,
                         justify="left", font=("Segoe UI", 9)).pack(fill="x", pady=5)
                changed = True
            elif kind == "error":
                self.phase_text = value
                self._status_error = True
                self.awaiting_empty_note = False
                self.status.configure(text=value, fg=ERROR)
                self.show_note(value, ERROR)
                if self.answer_text is not None and self.answer_text.winfo_exists() and not self.answer_buffer:
                    self.answer_text.configure(text="The answer stopped before it finished.")
                if self.job_kind == "index":
                    self.set_library_status("Indexing stopped: " + value, ERROR)
                changed = True
            elif kind == "done" and isinstance(value, dict) and "model_ready" in value:
                self.phase_text = value["model_ready"] + " ready · choose it in Settings."
                self._status_error = False
                self.model_ready_pending = True
            elif kind == "done" and isinstance(value, dict) and "indexed" in value:
                summary = f"{value['indexed']} updated, {value['unchanged']} unchanged, {value['deleted']} removed"
                if value.get("skipped"):
                    summary += f", {value['skipped']} skipped"
                if value.get("unreadable"):
                    summary += f", {value['unreadable']} unreadable"
                self.phase_text = "Ready · " + summary
                self._status_error = False
                detail = format_dropped(value)
                shown = summary if not detail else summary + "\n" + detail
                self.set_library_status("Indexing complete · " + shown, ACCENT)
                if detail:
                    self.show_note(detail, MUTED)
            elif kind == "idle":
                cancelled = isinstance(value, dict) and bool(value.get("cancelled"))
                self.busy = False
                self.job_kind = None
                self.hide_stop()
                self.finish_answer()
                self.restore_compact()
                if self.phase_text:
                    self.status.configure(text=self.phase_text, fg=ERROR if self._status_error else MUTED)
                self.entry.configure(state="normal")
                changed = True
                if self.engine is not None:
                    self.library_label.configure(text=f"Your library · {self.engine.count()} files · On device")
                if self.library_window and self.library_window.winfo_exists():
                    self.refresh_library_list()
                self.set_library_topmost(False)
                if self.pending_folders and not cancelled:
                    self.start_next_pending_folder()
                elif cancelled and self.pending_folders:
                    waiting = len(self.pending_folders)
                    noun = "folder" if waiting == 1 else "folders"
                    self.set_library_status(f"Stopped. {waiting} {noun} still waiting — select one and choose Refresh.", MUTED)
                    self.refresh_library_list()
                if getattr(self, "model_ready_pending", False) and not self.busy:
                    self.model_ready_pending = False
                    self.show_settings()
                    self.status.configure(text=self.phase_text, fg=ACCENT)
        self.paint_activity()
        if changed:
            self.fit_content()

    def show_answer(self):
        if self.answer_window and self.answer_window.winfo_exists():
            self.answer_window.lift()
            return
        self.answer_window = tk.Toplevel(self.root)
        self.answer_window.title("Loft · Local answer")
        self.answer_window.configure(bg=BG)
        self.answer_window.attributes("-topmost", True)
        self.place_dialog(self.answer_window, 700, 500)
        self.full_answer = scrolledtext.ScrolledText(self.answer_window, bg=BG, fg=TEXT,
            wrap="word", padx=24, pady=24, font=("Segoe UI", 12))
        self.full_answer.pack(fill="both", expand=True)
        self.full_answer.insert("1.0", self.answer_buffer)
        self.mark_unsupported(self.full_answer, self.answer_buffer)
        self.full_answer.configure(state="disabled")
        if self.support_gaps:
            self.answer_window.title("Loft · Local answer · not fully supported")
            self.answer_gap_note = tk.Label(self.answer_window, text=self.support_line(), bg=BG, fg=ERROR,
                            wraplength=640, justify="left", anchor="w", font=("Segoe UI", 10), padx=16, pady=8)
            self.answer_gap_note.pack(anchor="w", before=self.full_answer)

    def manage_library(self):
        indexing = bool(self.busy and self.job_kind == "index")
        if self.library_window and self.library_window.winfo_exists():
            self.set_library_topmost(indexing)
            self.library_window.lift()
            return
        window = self.library_window = tk.Toplevel(self.root)
        window.title("Loft · Your folders")
        window.configure(bg=BG)
        self.library_topmost = indexing
        window.attributes("-topmost", indexing)
        self.place_dialog(window, 680, 380)
        tk.Label(window, text="Choose what Loft can search", bg=BG, fg=TEXT, font=("Segoe UI", 16), padx=20, pady=20).pack(anchor="w")
        tk.Label(window, text="Text, Markdown, PDF and code · stored locally · refresh after file changes",
                 bg=BG, fg=MUTED, padx=20).pack(anchor="w")
        self.folder_list = tk.Listbox(window, bg=PANEL, fg=TEXT, relief="flat", selectbackground=HOVER)
        self.folder_list.pack(fill="both", expand=True, padx=20, pady=15)
        self.library_status = tk.Label(window, text="Ready to add or refresh a folder", bg=BG, fg=MUTED,
                                       anchor="w", justify="left", wraplength=620, padx=20, font=("Segoe UI", 10))
        self.library_status.pack(fill="x", pady=(0, 8))
        actions = tk.Frame(window, bg=BG)
        actions.pack(fill="x", padx=20, pady=(0, 18))
        add_button = self.button(actions, "+ Add folder", self.add_folder)
        add_button.configure(bg=ACCENT, fg=BG, activebackground=HOVER, activeforeground=TEXT)
        add_button.pack(side="left", padx=(0, 8))
        self.button(actions, "Refresh selected", self.refresh_folder).pack(side="left")
        self.button(actions, "Remove selected", self.remove_folder).pack(side="left")
        self.button(actions, "Stop", lambda: self.cancel_work.set()).pack(side="right")
        self.refresh_library_list()
        if self.busy and self.job_kind == "index":
            self.set_library_status("Indexing… " + self.phase_text, ACCENT)

    def set_library_status(self, message, color=MUTED):
        if self.library_window and self.library_window.winfo_exists():
            self.library_status.configure(text=message, fg=color)

    def set_library_topmost(self, enabled):
        self.library_topmost = bool(enabled)
        if self.library_window and self.library_window.winfo_exists():
            try:
                self.library_window.attributes("-topmost", self.library_topmost)
            except tk.TclError:
                pass

    def refresh_library_list(self):
        if not (self.library_window and self.library_window.winfo_exists()):
            return
        selected = self.selected_folder()
        self.folder_list.delete(0, "end")
        self.folder_rows = []
        seen = set()
        for folder in self.engine.roots():
            count = self.engine.root_count(folder)
            noun = "file" if count == 1 else "files"
            label = f"{folder}    ·    {count} {noun}"
            if getattr(self.engine, "root_is_stale", None) and self.engine.root_is_stale(folder):
                label += "    ·    changed · refresh to update"
            self.folder_list.insert("end", label)
            self.folder_rows.append(folder)
            seen.add(folder)
        for folder in self.pending_folders:
            if folder in seen:
                continue
            self.folder_list.insert("end", f"{folder}    ·    waiting to index")
            self.folder_rows.append(folder)
        if selected:
            self.select_folder_row(selected)

    def selected_folder(self):
        if not (self.library_window and self.library_window.winfo_exists()):
            return None
        selected = self.folder_list.curselection()
        if not selected or selected[0] >= len(self.folder_rows):
            return None
        return self.folder_rows[selected[0]]

    def select_folder_row(self, folder):
        if folder in self.folder_rows and self.library_window and self.library_window.winfo_exists():
            index = self.folder_rows.index(folder)
            self.folder_list.selection_clear(0, "end")
            self.folder_list.selection_set(index)
            self.folder_list.see(index)

    def folder_error(self, message):
        self._status_error = True
        self.phase_text = message
        self.status.configure(text=message, fg=ERROR)
        self.set_library_status(message, ERROR)

    def pending_overlap(self, folder):
        root = Path(folder)
        for pending in self.pending_folders:
            other = Path(pending)
            if root != other and (root.is_relative_to(other) or other.is_relative_to(root)):
                return True
        return False

    def add_folder(self):
        folder = filedialog.askdirectory(parent=self.library_window, title="Choose a folder for Loft")
        if folder:
            self.include_folder(folder)

    def include_folder(self, folder):
        try:
            folder = self.engine.normalize_root(folder)
        except (OSError, ValueError) as exc:
            self.folder_error(str(exc))
            return
        if self.pending_overlap(folder):
            self.folder_error("That folder overlaps a folder that is waiting to index.")
            return
        if folder in self.pending_folders:
            self.set_library_status("That folder is already waiting to index.", MUTED)
            return
        if self.busy:
            self.pending_folders.append(folder)
            self.refresh_library_list()
            self.note_phase("Folder added. It will index when the current task finishes.")
            self.set_library_status("Folder added · waiting to index…", ACCENT)
            return
        self.index_now(folder)

    def index_now(self, folder):
        try:
            folder = self.engine.add_root(folder)
        except (OSError, ValueError) as exc:
            self.folder_error(str(exc))
            return
        except sqlite3.OperationalError:
            if folder not in self.pending_folders:
                self.pending_folders.append(folder)
            self.refresh_library_list()
            self.folder_error("The library is busy. The folder will index when it is free.")
            return
        self.refresh_library_list()
        self.select_folder_row(folder)
        self.start_index(folder)

    def start_index(self, folder):
        if self.start_job(lambda cancel, emit, folder=folder: self.engine.index_folder(folder, cancel, emit), kind="index"):
            self.show_stop()
            self.set_library_topmost(True)
            if self.library_window and self.library_window.winfo_exists():
                self.library_window.lift()
            self.note_phase("Loading the search model…")
            self.set_library_status("Indexing… loading the search model", ACCENT)
            return
        if folder not in self.pending_folders:
            self.pending_folders.insert(0, folder)
        self.refresh_library_list()

    def start_next_pending_folder(self):
        while self.pending_folders:
            folder = self.pending_folders.pop(0)
            try:
                folder = self.engine.add_root(folder)
            except (OSError, ValueError) as exc:
                self.folder_error(str(exc))
                continue
            except sqlite3.OperationalError:
                self.pending_folders.insert(0, folder)
                self.refresh_library_list()
                self.folder_error("The library is busy. Select the folder and refresh to try again.")
                return
            self.refresh_library_list()
            self.select_folder_row(folder)
            self.start_index(folder)
            return
        self.refresh_library_list()

    def refresh_folder(self):
        folder = self.selected_folder()
        if folder is None:
            self.set_library_status("Select a folder, then refresh it.", MUTED)
            return
        if self.busy:
            self.set_library_status("An operation is running. Use Stop, then refresh.", MUTED)
            return
        if folder in self.pending_folders:
            self.pending_folders.remove(folder)
        self.index_now(folder)

    def remove_folder(self):
        folder = self.selected_folder()
        if folder is None:
            self.set_library_status("Select a folder first.", MUTED)
            return
        if self.busy:
            self.set_library_status("An operation is running. Use Stop, then remove the folder.", MUTED)
            return
        if folder in self.pending_folders and folder not in self.engine.roots():
            if not self.confirm_remove(folder, 0, waiting=True):
                return
            self.pending_folders.remove(folder)
            self.refresh_library_list()
            self.set_library_status("Removed the folder that was waiting to index.", MUTED)
            return
        count = self.engine.root_count(folder) if folder in self.engine.roots() else 0
        if not self.confirm_remove(folder, count):
            return
        self.engine.remove(folder)
        if folder in self.pending_folders:
            self.pending_folders.remove(folder)
        self.refresh_library_list()
        self.library_label.configure(text=f"Your library · {self.engine.count()} files · On device")
        self.clear_results()
        self._status_error = False
        noun = "file" if count == 1 else "files"
        self.status.configure(text=f"Folder removed from Loft. {count} indexed {noun} are no longer searchable. Original files are unchanged.", fg=MUTED)
        self.fit_content()

    def confirm_remove(self, folder, count, waiting=False):
        if waiting:
            message = "This folder is waiting to index and is not searchable yet. Remove it from the list?"
        else:
            noun = "file" if count == 1 else "files"
            message = (f"Remove this folder from search?\n\n{count} indexed {noun} will no longer be searchable. "
                       "Original files stay on disk.")
        return messagebox.askyesno("Loft · Remove folder", message, parent=self.library_window)

    def update_placeholder(self, *_):
        if self.question.get():
            self.placeholder.place_forget()
        else:
            self.placeholder.place(x=0, rely=0.5, anchor="w")

    def refresh_theme(self):
        self.apply_theme(windows_palette())
        self.theme_timer = self.root.after(3000, self.refresh_theme)

    def apply_theme(self, colours):
        global BG, PANEL, TEXT, MUTED, ACCENT, HOVER
        previous = (BG, PANEL, TEXT, MUTED, ACCENT, HOVER)
        if colours == previous:
            return
        replacements = dict(zip(previous, colours))
        def recolour(widget):
            for option in ("background", "foreground", "activebackground", "activeforeground",
                           "insertbackground", "highlightbackground", "highlightcolor"):
                if option in widget.keys():
                    old = str(widget.cget(option))
                    if old in replacements:
                        widget.configure(**{option: replacements[old]})
            for child in widget.winfo_children():
                recolour(child)
        recolour(self.root)
        BG, PANEL, TEXT, MUTED, ACCENT, HOVER = colours
        self.canvas.configure(bg=BG)
        self.input_surface.draw()
        self.style_scopes()

    def button(self, parent, text, command, muted=False):
        return tk.Button(parent, text=text, command=command, bg=parent.cget("bg"), fg=MUTED if muted else TEXT,
                         activebackground=HOVER, activeforeground=TEXT, relief="flat", bd=0,
                         padx=8, pady=3, cursor="hand2", font=("Segoe UI", 10),
                         highlightthickness=0, highlightbackground=BG, highlightcolor=ACCENT)

    def style_scopes(self):
        for name, button in self.scope_buttons.items():
            if name == self.scope:
                button.configure(bg=ACCENT, fg=BG, activebackground=ACCENT, activeforeground=BG)
            else:
                button.configure(bg=BG, fg=MUTED, activebackground=HOVER, activeforeground=TEXT)

    def focus_search(self):
        self.expand()
        self.entry.focus_set()

    def load_prefs(self):
        path = ROOT / "data" / "window.json"
        try:
            return json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            return {}

    def save_prefs(self):
        if not self.use_settings:
            return
        path = ROOT / "data" / "window.json"
        path.parent.mkdir(parents=True, exist_ok=True)
        payload = {"glass": self.glass, "opacity": self.opacity, "topmost": self.topmost}
        if self.origin is not None:
            payload["x"], payload["y"] = (int(self.origin[0]), int(self.origin[1]))
        path.write_text(json.dumps(payload), encoding="utf-8")

    def bind_drag(self, widget, on_click=None):
        widget.bind("<ButtonPress-1>", self.drag_start)
        widget.bind("<B1-Motion>", self.drag_move)
        widget.bind("<ButtonRelease-1>", lambda event, action=on_click: self.drag_end(event, action))

    def drag_start(self, event):
        self._press = (event.x_root, event.y_root, self.root.winfo_x(), self.root.winfo_y(), False)

    def drag_move(self, event):
        if not self._press:
            return
        x0, y0, wx, wy, _moved = self._press
        if abs(event.x_root - x0) + abs(event.y_root - y0) < 4:
            return
        self._press = (x0, y0, wx, wy, True)
        self.place_window(wx + event.x_root - x0, wy + event.y_root - y0, remember=False)

    def drag_end(self, event, on_click):
        moved = bool(self._press and self._press[4])
        self._press = None
        if moved:
            self.save_prefs()
        elif on_click:
            on_click()

    def place_dialog(self, window, width, height):
        self.root.update_idletasks()
        x = self.root.winfo_x()
        y = self.root.winfo_y() + max(self.root.winfo_height(), 48) + 8
        left, top, right, bottom = virtual_bounds(monitors())
        width = min(width, max(280, right - left - 40))
        height = min(height, max(180, bottom - top - 40))
        if y + height > bottom:
            y = max(top + 8, bottom - height - 8)
        if x + width > right:
            x = max(left + 8, right - width - 8)
        window.geometry(f"{int(width)}x{int(height)}+{int(max(left, x))}+{int(max(top, y))}")
        window.bind("<Escape>", lambda _event: window.destroy())

    def place_window(self, x, y, remember=True):
        screens = monitors()
        left, top, right, bottom = virtual_bounds(screens)
        width, height = max(self.root.winfo_width(), 40), max(self.root.winfo_height(), 40)
        x = int(min(max(x, left), right - min(80, width)))
        y = int(min(max(y, top), bottom - min(40, height)))
        self.origin = (x, y)
        self.root.geometry(f"+{x}+{y}")
        if remember:
            self.save_prefs()

    def move_to_screen(self, screen):
        width, height = self.root.winfo_width(), self.root.winfo_height()
        x = screen["left"] + max(0, (screen["right"] - screen["left"] - width) // 2)
        self.place_window(x, screen["top"])

    def set_glass(self, enabled):
        self.glass = enabled
        self.apply_material()
        self.save_prefs()
        self.show_settings()

    def set_opacity(self, value):
        self.opacity = float(value)
        if self.glass:
            self.root.attributes("-alpha", self.opacity)
        self.save_prefs()

    def set_topmost(self):
        self.topmost = not self.topmost
        self.root.attributes("-topmost", self.topmost)
        self.save_prefs()
        self.show_settings()

    def apply_material(self):
        self.root.update_idletasks()
        apply_window_material(self.root, self.root.winfo_width(), self.root.winfo_height(), self.glass, self.opacity)

    def show_settings(self):
        if self.busy:
            self.note_phase("Finish or stop the current operation before changing settings.")
            return
        if self.library_window and self.library_window.winfo_exists():
            self.library_window.destroy()
        if not self.expanded:
            self.expand()
        self.clear_results()
        self.status.configure(text="Settings", fg=TEXT)
        self.present_model_settings()
        tk.Label(self.results, text="APPEARANCE", bg=BG, fg=ACCENT,
                 font=("Segoe UI", 9, "bold")).pack(anchor="w", pady=(14, 4))
        appearance = tk.Frame(self.results, bg=BG)
        appearance.pack(fill="x", pady=(2, 0))
        self.button(appearance, "Glass", lambda: self.set_glass(True)).pack(side="left", padx=(0, 4))
        self.button(appearance, "Solid", lambda: self.set_glass(False)).pack(side="left")
        for name, button in (("Glass", appearance.winfo_children()[0]), ("Solid", appearance.winfo_children()[1])):
            chosen = (name == "Glass") == self.glass
            button.configure(bg=ACCENT if chosen else BG, fg=BG if chosen else MUTED)
        if self.glass:
            scale = tk.Scale(self.results, from_=0.55, to=0.92, resolution=0.01, orient="horizontal",
                             bg=BG, fg=TEXT, troughcolor=PANEL, highlightthickness=0, length=240,
                             showvalue=False, command=self.set_opacity)
            scale.set(self.opacity)
            scale.pack(anchor="w", pady=(4, 0))
        staying = self.button(self.results, "Always on top" if self.topmost else "Not always on top", self.set_topmost, muted=not self.topmost)
        staying.pack(anchor="w", pady=(4, 0))
        screens = tk.Frame(self.results, bg=BG)
        screens.pack(fill="x", pady=(4, 0))
        for index, screen in enumerate(monitors(), 1):
            label = f"Screen {index}" + (" · primary" if screen["primary"] else "")
            self.button(screens, label, lambda s=screen: self.move_to_screen(s), muted=True).pack(side="left", padx=(0, 4))
        self.button(self.results, "← Back to search", self.show_welcome, muted=True).pack(anchor="w", pady=(8, 0))
        self.fit_content()

    def present_model_settings(self):
        if self.demo:
            return
        models_api = getattr(self.engine, "models", None)
        if models_api is None:
            return
        tk.Label(self.results, text="ANSWERS & SUMMARIES", bg=BG, fg=ACCENT,
                 font=("Segoe UI", 9, "bold")).pack(anchor="w", pady=(2, 4))
        if not getattr(models_api, "manifest", None):
            tk.Label(self.results, text="No local models yet. Run setup_runtime.py, then choose an answer model here.",
                     bg=BG, fg=MUTED, wraplength=480, justify="left", font=("Segoe UI", 9)).pack(anchor="w", pady=(0, 6))
        else:
            embed = models_api.embedding_record() if hasattr(models_api, "embedding_record") else None
            if embed:
                ready = models_api.model_path(embed["file"]).is_file()
                detail = "ready" if ready else "missing — run setup_runtime.py"
                tk.Label(self.results, text=f"File search embeddings · {embed['file']} · {detail}",
                         bg=BG, fg=ACCENT if ready else ERROR, wraplength=480, justify="left",
                         font=("Segoe UI", 9)).pack(anchor="w", pady=(0, 6))
            else:
                tk.Label(self.results, text="File search embeddings are missing. Run setup_runtime.py before indexing.",
                         bg=BG, fg=ERROR, wraplength=480, justify="left", font=("Segoe UI", 9)).pack(anchor="w", pady=(0, 6))
        try:
            models = models_api.answer_models()
            selected = models_api.selected_answer() if models else None
        except (RuntimeError, KeyError, ValueError, OSError):
            models, selected = [], None
        installed = {model["file"]: model for model in models}
        for candidate in models_api.CATALOG:
            title, description, info = candidate
            filename = info[2]
            row = tk.Frame(self.results, bg=PANEL)
            row.pack(fill="x", pady=2)
            detail = "In use" if selected and selected["file"] == filename else ("Downloaded" if filename in installed else description)
            tk.Label(row, text=f"{title}  ·  {detail}", bg=PANEL, fg=TEXT,
                     font=("Segoe UI", 10), anchor="w").pack(side="left", padx=9, pady=6)
            if filename in installed:
                if not (selected and selected["file"] == filename):
                    self.button(row, "Use", lambda f=filename: self.select_answer_model(f)).pack(side="right", padx=5)
            else:
                exists = models_api.model_path(filename).is_file()
                if exists:
                    self.button(row, "Add local file",
                                lambda c=candidate: self.add_existing_model(c)).pack(side="right", padx=5)
                else:
                    self.button(row, "Download",
                                lambda c=candidate: self.download_model(c)).pack(side="right", padx=5)
        for model in models:
            if model["file"] in {candidate[2][2] for candidate in models_api.CATALOG}:
                continue
            row = tk.Frame(self.results, bg=PANEL)
            row.pack(fill="x", pady=2)
            title = model.get("title", model["file"])
            detail = "In use" if selected and selected["file"] == model["file"] else "Downloaded"
            tk.Label(row, text=f"{title}  ·  {detail}", bg=PANEL, fg=TEXT,
                     font=("Segoe UI", 10), anchor="w").pack(side="left", padx=9, pady=6)
            if detail != "In use":
                self.button(row, "Use", lambda f=model["file"]: self.select_answer_model(f)).pack(side="right", padx=5)
        self.button(self.results, "+ Add a GGUF model", self.import_model, muted=True).pack(anchor="w", pady=(4, 0))
        tk.Label(self.results, text="Search uses a separate embedding model. Switching this model keeps your index.",
                 bg=BG, fg=MUTED, wraplength=480, justify="left", font=("Segoe UI", 9)).pack(anchor="w", pady=(2, 0))

    def show_models(self):
        self.show_settings()

    def download_model(self, candidate):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        models = getattr(self.engine, "models", None)
        if models is not None and not getattr(models, "manifest", None):
            self._status_error = True
            self.phase_text = "Local models are not installed. Run setup_runtime.py first."
            self.status.configure(text=self.phase_text, fg=ERROR)
            return
        if self.start_job(lambda cancel, emit: self.engine.models.download_answer(candidate, cancel, emit), kind="model"):
            self.show_stop()

    def add_existing_model(self, candidate):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        filename = self.engine.models.model_path(candidate[2][2])
        if self.start_job(lambda cancel, emit: self.engine.models.import_answer(filename, cancel, emit), kind="model"):
            self.show_stop()

    def import_model(self):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        models = getattr(self.engine, "models", None)
        if models is not None and not getattr(models, "manifest", None):
            self._status_error = True
            self.phase_text = "Local models are not installed. Run setup_runtime.py first."
            self.status.configure(text=self.phase_text, fg=ERROR)
            return
        filename = filedialog.askopenfilename(parent=self.root, title="Choose an answer model", filetypes=[("GGUF models", "*.gguf")])
        if filename and self.start_job(lambda cancel, emit: self.engine.models.import_answer(filename, cancel, emit), kind="model"):
            self.show_stop()

    def select_answer_model(self, filename):
        if self.busy:
            self.note_phase("An operation is running. Use Stop, then try again.")
            return
        try:
            self.engine.models.select_answer(filename)
        except (OSError, ValueError, RuntimeError) as exc:
            self._status_error = True
            self.phase_text = str(exc)
            self.status.configure(text=str(exc), fg=ERROR)
            return
        self.show_settings()
        self.status.configure(text="Saved · this model will answer your next question.", fg=ACCENT)

    def layout(self, width, height):
        screens = monitors()
        if self.origin is None:
            home = next(screen for screen in screens if screen["primary"])
            width = min(width, home["right"] - home["left"] - 20)
            height = min(height, home["bottom"] - home["top"] - 20)
            x = home["left"] + (home["right"] - home["left"] - width) // 2
            y = home["top"]
        else:
            x, y = self.origin
            width = min(width, 1200)
            height = min(height, 900)
        self.root.geometry(f"{int(width)}x{int(height)}+{int(x)}+{int(y)}")
        self.canvas.configure(bg=BG)
        self.canvas.itemconfigure(self.body_id, width=width-48, height=0)
        self.apply_material()

    def fit_content(self):
        if self.closed or not self.expanded:
            return
        try:
            if not self.root.winfo_exists():
                return
        except tk.TclError:
            return
        self.cancel_animation()
        self.canvas.itemconfigure(self.body_id, width=512)
        self.root.update_idletasks()
        self.size_results_viewport()
        self.root.update_idletasks()
        self.layout(560, self.body.winfo_reqheight()+42)
        self.finish_transition()

    def size_results_viewport(self):
        """Grow the results area with its content, and scroll once the screen is full."""
        self.root.update_idletasks()
        inner = max(self.scroll_inner.winfo_reqheight(), 1)
        chrome = max(self.body.winfo_reqheight() - int(self.scroll.cget("height")), 0)
        screens = monitors()
        if self.origin is None:
            home = next(screen for screen in screens if screen["primary"])
            limit = home["bottom"] - home["top"] - 20
        else:
            limit = 900
        viewport = max(1, min(inner, max(80, limit - 42 - chrome)))
        self.scroll.configure(height=viewport)
        width = max(self.scroll.winfo_width(), 480)
        self.scroll.itemconfigure(self.scroll_window, width=width)
        self.scroll.configure(scrollregion=(0, 0, width, inner))
        overflowing = inner > viewport + 2
        if overflowing and not self.vbar.winfo_ismapped():
            self.vbar.pack(side="right", fill="y")
        elif not overflowing and self.vbar.winfo_ismapped():
            self.vbar.pack_forget()
            self.scroll.yview_moveto(0)

    def _sync_scroll_width(self, event=None):
        if event is not None and event.width > 1:
            self.scroll.itemconfigure(self.scroll_window, width=event.width)

    def _on_results_scroll(self, first, last):
        self.vbar.set(first, last)

    def _pointer_over_results(self):
        widget = self.root.winfo_containing(self.root.winfo_pointerx(), self.root.winfo_pointery())
        while widget is not None:
            if widget == self.scroll_host:
                return True
            widget = getattr(widget, "master", None)
        return False

    def _on_results_wheel(self, event):
        if not self.expanded or not self.vbar.winfo_ismapped() or not self._pointer_over_results():
            return
        if getattr(event, "num", None) == 4 or getattr(event, "delta", 0) > 0:
            self.scroll.yview_scroll(-1, "units")
        else:
            self.scroll.yview_scroll(1, "units")
        return "break"

    def cancel_animation(self):
        if self.animation_timer is not None:
            self.root.after_cancel(self.animation_timer)
            self.animation_timer = None

    def finish_transition(self):
        self.canvas.itemconfigure(self.compact_id, state="hidden" if self.expanded else "normal")
        self.canvas.itemconfigure(self.body_id, state="normal" if self.expanded else "hidden")
        if self.expanded:
            self.entry.focus_set()

    def transition(self, width, height):
        self.cancel_animation()
        if not self.motion_ready or not motion_enabled():
            self.layout(width, height)
            self.finish_transition()
            return
        start_width, start_height = self.root.winfo_width(), self.root.winfo_height()
        started = time.perf_counter()
        self.canvas.itemconfigure(self.body_id, state="hidden")
        self.canvas.itemconfigure(self.compact_id, state="hidden")
        def frame():
            self.animation_timer = None
            progress = min(1.0, (time.perf_counter()-started)/0.24)
            eased = 1 - (1-progress)**3
            self.layout(round(start_width+(width-start_width)*eased),
                        round(start_height+(height-start_height)*eased))
            if progress < 1:
                self.animation_timer = self.root.after(16, frame)
            else:
                self.finish_transition()
        frame()

    def expand(self):
        if self.expanded:
            return
        self.expanded = True
        self.canvas.itemconfigure(self.body_id, width=512)
        self.root.update_idletasks()
        self.size_results_viewport()
        self.root.update_idletasks()
        self.transition(560, self.body.winfo_reqheight()+42)
        self.root.lift()

    def collapse(self):
        self.expanded = False
        self.transition(285, 48)

    def clear_results(self):
        # Recreate the container: Tk retains a frame's last requested height
        # after its last packed child is removed, leaving an oversized void.
        self.answer_card = None
        self.answer_text = None
        self.answer_buffer = ""
        self.support_gaps = []
        self.support_note = None
        previous = self.results
        self.results = tk.Frame(self.scroll_inner, bg=BG, height=1)
        self.results.pack(fill="x", before=previous)
        previous.destroy()
        self.scroll.yview_moveto(0)

    def present_card(self, caption, text):
        card = tk.Frame(self.results, bg=HOVER, padx=12, pady=8)
        card.pack(fill="x")
        tk.Label(card, text=caption, bg=HOVER, fg=ACCENT, anchor="w",
                 font=("Segoe UI", 9)).pack(anchor="w")
        self.mount_rich(card, text)

    def present_streaming(self):
        self.answer_card = tk.Frame(self.results, bg=HOVER, padx=12, pady=8)
        self.answer_card.pack(fill="x")
        tk.Label(self.answer_card, text="Local answer", bg=HOVER, fg=ACCENT, anchor="w",
                 font=("Segoe UI", 9)).pack(anchor="w")
        self.answer_buffer = ""
        self.answer_text = tk.Label(self.answer_card, text="Writing an answer…", bg=HOVER, fg=TEXT, wraplength=460,
                                     justify="left", anchor="w", font=("Segoe UI", 11), cursor="hand2")
        self.answer_text.pack(anchor="w", fill="x", pady=(4, 0))
        self.answer_text.bind("<Button-1>", lambda _event: self.show_answer())

    def support_line(self):
        if not self.support_gaps:
            return ""
        return "Not in the cited passage: " + ", ".join(self.support_gaps)

    def mark_unsupported(self, box, answer):
        if not self.support_gaps or not answer:
            return
        box.tag_configure("gap", foreground=ERROR, underline=True)
        wanted = {gap.lower() for gap in self.support_gaps}
        for match in re.finditer(r"\w+", answer):
            if match.group().lower() in wanted:
                box.tag_add("gap", f"1.0+{match.start()}c", f"1.0+{match.end()}c")

    def show_support_flag(self):
        line = self.support_line()
        if not line:
            return
        card = self.answer_card
        if card is not None and card.winfo_exists():
            children = card.winfo_children()
            if children:
                children[0].configure(text="Local answer · not fully supported", fg=ERROR)
            current = self.answer_text
            if isinstance(current, tk.Label) and current.winfo_exists():
                current.destroy()
                self.answer_text = self.mount_flagged_answer(card, self.answer_buffer)
            if self.support_note is None or not self.support_note.winfo_exists():
                self.support_note = tk.Label(card, text=line, bg=HOVER, fg=ERROR, wraplength=460,
                                              justify="left", anchor="w", font=("Segoe UI", 9))
                self.support_note.pack(anchor="w", fill="x", pady=(6, 0))
            else:
                self.support_note.configure(text=line)
        window = self.answer_window
        if window is not None and window.winfo_exists() and getattr(self, "full_answer", None):
            window.title("Loft · Local answer · not fully supported")
            self.full_answer.configure(state="normal")
            self.mark_unsupported(self.full_answer, self.answer_buffer)
            self.full_answer.configure(state="disabled")
            if not getattr(self, "answer_gap_note", None) or not self.answer_gap_note.winfo_exists():
                self.answer_gap_note = tk.Label(window, text=line, bg=BG, fg=ERROR, wraplength=640,
                                                 justify="left", anchor="w", font=("Segoe UI", 10), padx=16, pady=8)
                self.answer_gap_note.pack(anchor="w", before=self.full_answer)
            else:
                self.answer_gap_note.configure(text=line)

    def mount_flagged_answer(self, parent, answer):
        lines = max(1, answer.count("\n") + (len(answer) // 55) + 1)
        box = tk.Text(parent, height=min(lines, 8), wrap="word", bg=HOVER, fg=TEXT, relief="flat",
                      font=("Segoe UI", 11), highlightthickness=0, takefocus=0, cursor="hand2", padx=0, pady=0)
        box.pack(anchor="w", fill="x", pady=(4, 0))
        box.insert("1.0", answer)
        self.mark_unsupported(box, answer)
        box.configure(state="disabled")
        box.bind("<Button-1>", lambda _event: self.show_answer())
        return box

    def finish_answer(self):
        card = self.answer_card
        if not self.answer_buffer or card is None or not card.winfo_exists():
            return
        if "```" in self.answer_buffer:
            for child in list(card.winfo_children())[1:]:
                child.destroy()
            self.mount_rich(card, self.answer_buffer)
            self.answer_text = None
            self.support_note = None
        self.show_support_flag()

    def parts(self, text):
        if "```" not in text:
            if re.match(r"\s*(def |class )", text):
                return [("code", text, "python")]
            return [("text", text)]
        pattern = re.compile(r"```([^\n`]*)\n?(.*?)```", re.S)
        chunks, last = [], 0
        for match in pattern.finditer(text):
            if match.start() > last:
                chunks.append(("text", text[last:match.start()]))
            chunks.append(("code", match.group(2), match.group(1).strip() or "code"))
            last = match.end()
        if last < len(text):
            chunks.append(("text", text[last:]))
        return chunks or [("text", text)]

    def mount_rich(self, parent, text):
        for kind, *rest in self.parts(text):
            if kind == "text":
                body = rest[0].strip()
                if body:
                    tk.Label(parent, text=body, bg=HOVER, fg=TEXT, wraplength=460, justify="left",
                             anchor="w", font=("Segoe UI", 11)).pack(anchor="w", fill="x", pady=(4, 0))
            else:
                self.add_code_block(parent, rest[0], rest[1])

    def add_code_block(self, parent, code, language):
        code = code.strip("\n")
        block = tk.Frame(parent, bg=PANEL)
        block.pack(fill="x", pady=(6, 0))
        bar = tk.Frame(block, bg=PANEL)
        bar.pack(fill="x")
        tk.Label(bar, text=language or "code", bg=PANEL, fg=MUTED,
                 font=("Segoe UI", 8)).pack(side="left", padx=(8, 0))
        copy = self.button(bar, "Copy", lambda: None, muted=True)
        copy.configure(command=lambda c=code, b=copy: self.copy_code(c, b))
        copy.pack(side="right")
        lines = code.splitlines() or [""]
        box = tk.Text(block, height=min(len(lines), 6), width=48, bg=PANEL, fg=TEXT, relief="flat",
                      bd=0, padx=8, pady=4, wrap="word", font=("Consolas", 10),
                      takefocus=0, highlightthickness=0)
        box.pack(fill="x", padx=8, pady=(0, 6))
        self.highlight(box, code)
        box.configure(state="disabled")

    def highlight(self, box, code):
        box.insert("1.0", code)
        box.tag_configure("kw", foreground="#8ab4f8")
        box.tag_configure("str", foreground="#9ece6a")
        box.tag_configure("com", foreground=MUTED)
        for line_no, line in enumerate(code.splitlines(), 1):
            for match in re.finditer(r"#.*$", line):
                box.tag_add("com", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")
            for match in re.finditer(r"(\"(?:\\.|[^\"\\])*\"|'(?:\\.|[^'\\])*')", line):
                box.tag_add("str", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")
            for match in re.finditer(r"\b[A-Za-z_][A-Za-z0-9_]*\b", line):
                if match.group() in CODE_WORDS:
                    box.tag_add("kw", f"{line_no}.{match.start()}", f"{line_no}.{match.end()}")

    def copy_code(self, code, button):
        try:
            self.root.clipboard_clear()
            self.root.clipboard_append(code)
            self.root.update_idletasks()
        except tk.TclError:
            self.status.configure(text="Could not copy to the clipboard.", fg=ERROR)
            return
        button.configure(text="Copied")
        self.root.after(1200, lambda: button.configure(text="Copy") if button.winfo_exists() else None)

    def present_sources(self, hits):
        self.sources_open = False
        wrap = tk.Frame(self.results, bg=BG)
        wrap.pack(fill="x", pady=(6, 0))
        self.sources_toggle = self.button(wrap, f"Sources · {len(hits)}  ▾", self.toggle_sources, muted=True)
        self.sources_toggle.pack(anchor="w")
        chips = tk.Frame(wrap, bg=BG)
        chips.pack(fill="x")
        for hit in hits:
            self.button(chips, f"[{hit['citation']}] {Path(hit['path']).name}",
                        lambda h=hit: self.open_source(h), muted=True).pack(side="left", padx=(0, 2))
        self.sources_detail = tk.Frame(wrap, bg=BG)
        for hit in hits:
            excerpt = hit["text"] if len(hit["text"]) <= 160 else hit["text"][:157] + "…"
            tk.Label(self.sources_detail, text=f"[{hit['citation']}]  {excerpt}", bg=BG, fg=MUTED,
                     wraplength=470, justify="left", anchor="w", font=("Segoe UI", 9)).pack(anchor="w", pady=(2, 0))
        if not self.demo:
            self.sources_open = True
            self.sources_detail.pack(fill="x", pady=(2, 0))
            self.sources_toggle.configure(text=f"Sources · {len(hits)}  ▴")

    def toggle_sources(self):
        self.sources_open = not self.sources_open
        count = len(self.last_hits)
        if self.sources_open:
            self.sources_detail.pack(fill="x", pady=(2, 0))
            self.sources_toggle.configure(text=f"Sources · {count}  ▴")
        else:
            self.sources_detail.pack_forget()
            self.sources_toggle.configure(text=f"Sources · {count}  ▾")
        self.fit_content()

    def show_welcome(self):
        self.clear_results()
        if not self.demo:
            self._status_error = False
            self.status.configure(text="Your files, ready when you are." if self.engine.count() else "Start with a folder of your own.", fg=MUTED)
            self.button(self.results, "Choose folders ↗", self.manage_library).pack(anchor="w", pady=8)
            return
        self.status.configure(text="A place to start")
        for label, question in (("Parking receipt", "Have I paid for parking today?"),
                                ("Laptop invoice", "Find the invoice for my laptop."),
                                ("Login code", "Where is login handled?")):
            row = tk.Frame(self.results, bg=BG)
            row.pack(fill="x", pady=2)
            self.button(row, label, lambda q=question: self.ask(q)).pack(side="left")
            self.button(row, "↗", lambda q=question: self.ask(q), muted=True).pack(side="right")

    def ask(self, question):
        self.question.set(question)
        self.search()

    def select_scope(self, scope):
        if scope == self.scope:
            return
        if self.busy and not self.demo:
            self.note_phase("An operation is running. Use Stop, then change mode.")
            return
        self.scope = scope
        self.style_scopes()
        self.placeholder.configure(text="Ask anything…" if scope == "Chat" else "Ask your files…")
        # Chat loads a model. Don't send just because the tab was clicked.
        if scope == "Chat":
            self._status_error = False
            self.pending_chat = ""
            self.clear_results()
            self.render_chat_transcript()
            self.note_phase("Chat doesn't use your files. Press Enter to send.")
            self.fit_content()
            return
        if self.question.get().strip():
            self.search()

    def search(self):
        if not self.demo:
            if self.busy:
                self.note_phase("An operation is running. Use Stop, then try again.")
                return
            question = self.question.get().strip()
            if not question:
                self.show_welcome()
                return
            if self.answer_window and self.answer_window.winfo_exists():
                self.answer_window.destroy()
            self.clear_results()
            scope = self.scope
            if scope == "Chat":
                self.pending_chat = question
                self.render_chat_transcript()
                history = [dict(turn) for turn in self.chat_history]
                operation = lambda cancel, emit, history=history: self.engine.chat_question(question, cancel, emit, history)
                kind, phase = "chat", "Loading the answer model…"
            else:
                operation = lambda cancel, emit: self.engine.ask(question, scope, cancel, emit)
                kind, phase = "ask", "Loading the search model…"
            if self.start_job(operation, kind=kind):
                self.show_stop()
                self.note_phase(phase)
            return
        self.root.after_idle(self.fit_content)
        question = self.question.get().strip()
        if not question:
            self.last_hits = []
            self.show_welcome()
            return
        self.clear_results()
        try:
            self.last_hits = self.searcher.search(question, self.scope)
        except (ValueError, RuntimeError) as exc:
            self.status.configure(text=f"Search could not finish: {exc}")
            return
        if not self.last_hits:
            self.status.configure(text="Nothing here yet.")
            tk.Label(self.results, text="Try parking, an invoice, or login code.\nThis preview searches five sample files.",
                     bg=BG, fg=MUTED, font=("Segoe UI", 11), justify="left").pack(anchor="w", pady=(4, 12))
            return
        self.status.configure(text="From your sample library")
        lead = self.last_hits[0]
        self.present_card(f"{Path(lead['path']).name} · lines {lead['start']}–{lead['end']}", lead["text"])
        self.present_sources(self.last_hits)

    def open_source(self, hit):
        if self.source_window and self.source_window.winfo_exists():
            self.source_window.destroy()
        window = self.source_window = tk.Toplevel(self.root)
        window.title(f"Loft · {Path(hit['path']).name} · {'sample' if self.demo else 'indexed'} passage")
        window.configure(bg=BG)
        window.attributes("-topmost", True)
        self.place_dialog(window, 680, 380)
        tk.Label(window, text=f"[{hit['citation']}]  {hit['path']}  ·  lines {hit['start']}–{hit['end']}",
                 bg=BG, fg=TEXT, font=("Segoe UI", 12), padx=16, pady=14).pack(anchor="w")
        tk.Label(window, text="Invented fixture · this is not an original file on your computer." if self.demo else hit["path"],
                 bg=BG, fg=ACCENT, padx=16).pack(anchor="w")
        text = scrolledtext.ScrolledText(window, bg=PANEL, fg=TEXT, wrap="word", relief="flat",
                                        padx=14, pady=14, font=("Consolas", 11))
        text.pack(fill="both", expand=True, padx=16, pady=16)
        text.insert("1.0", hit["text"])
        text.configure(state="disabled")
        if not self.demo:
            self.button(window, "Show original in folder", lambda: self.reveal_source(hit)).pack(anchor="w", padx=16, pady=(0, 12))
        window.bind("<Escape>", lambda _: window.destroy())

    def reveal_source(self, hit):
        path = Path(hit["path"])
        if not path.exists():
            self.status.configure(text="That source no longer exists. Refresh its folder.")
            return
        try:
            if sys.platform == "win32":
                subprocess.Popen(["explorer.exe", "/select,", str(path)])
            elif sys.platform == "darwin":
                subprocess.Popen(["open", "-R", str(path)])
            else:
                subprocess.Popen(["xdg-open", str(path.parent)])
        except OSError as exc:
            self.status.configure(text=f"Could not open the folder: {exc}", fg=ERROR)

    def render_chat_transcript(self):
        tk.Label(self.results, text="Chat · not based on your files", bg=BG, fg=ACCENT,
                 font=("Segoe UI", 9)).pack(anchor="w", pady=(4, 0))
        turns = list(self.chat_history)
        if self.pending_chat:
            turns.append({"role": "user", "content": self.pending_chat})
        if not turns:
            tk.Label(self.results, text="Press Enter to send. Earlier file answers are not part of this chat.",
                     bg=BG, fg=MUTED, wraplength=480, justify="left", anchor="w",
                     font=("Segoe UI", 10)).pack(anchor="w", pady=(6, 4))
            return
        for turn in turns[-6:]:
            who = "You" if turn["role"] == "user" else "Loft"
            tk.Label(self.results, text=f"{who}\n{turn['content']}", bg=BG, fg=TEXT, wraplength=480,
                     justify="left", anchor="w", font=("Segoe UI", 11)).pack(anchor="w", pady=(6, 0))

    def refresh_stale_markers(self):
        if self.demo or self.busy or not (self.library_window and self.library_window.winfo_exists()):
            return
        now = time.monotonic()
        if now - self._stale_checked < 3:
            return
        self._stale_checked = now
        self.refresh_library_list()

    def close(self):
        self.closed = True
        self.cancel_work.set()
        for sequence in ("<MouseWheel>", "<Button-4>", "<Button-5>"):
            self.root.unbind_all(sequence)
        if self.event_timer is not None:
            self.root.after_cancel(self.event_timer)
        self.cancel_animation()
        if self.theme_timer is not None:
            self.root.after_cancel(self.theme_timer)
        if self.searcher:
            self.searcher.close()
        if self.worker and self.worker.is_alive():
            self.worker.join(timeout=3)
        self.root.destroy()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true", help="Exercise UI flows and exit")
    parser.add_argument("--expanded", action="store_true")
    parser.add_argument("--demo", action="store_true", help="Use invented fixtures instead of your library")
    args = parser.parse_args()
    root = tk.Tk()
    app = LoftPreview(root, demo=args.demo or args.check, use_settings=not args.check)
    if args.expanded:
        app.expand()
    if args.check:
        def settle():
            done = tk.BooleanVar(root, False)
            root.after(300, lambda: done.set(True))
            root.wait_variable(done)
        app.expand()
        settle()
        root.update()
        app.ask("parking")
        root.update()
        assert app.last_hits[0]["path"] == "parking.txt"
        assert root.winfo_height() < 450, "Single result must not leave a large empty panel"
        assert app.body.winfo_height() >= app.body.winfo_reqheight(), "Content is clipped"
        app.open_source(app.last_hits[0])
        root.update()
        from loft_theme import palette
        for colour in ("#f3f3f3", "#1d5778", "#202020"):
            app.apply_theme(palette(colour))
            root.update()
            assert app.body.cget("bg") == palette(colour)[0]
            assert app.source_window.cget("bg") == palette(colour)[0]
            assert app.entry.get() == "parking"
        app.select_scope("Code")
        assert app.last_hits == []
        app.ask("login")
        assert app.last_hits[0]["path"] == "src/login.py"
        app.ask("!!!")
        root.update()
        assert root.winfo_height() < 380, "Empty state must shrink to its content"
        assert app.last_hits == []
        app.ask("")
        app.collapse()
        settle()
        root.update()
        assert not app.expanded
        assert root.winfo_height() == 48
        app.expand()
        root.after(40, app.collapse)
        root.after(80, app.expand)
        settle()
        settle()
        assert app.expanded and app.animation_timer is None
        assert app.canvas.itemcget(app.body_id, "state") == "normal"
        assert app.question.get() == ""
        from unittest.mock import patch
        with patch(__name__ + ".motion_enabled", return_value=False):
            app.collapse()
            root.update()
            assert app.animation_timer is None and root.winfo_height() == 48
            app.expand()
            assert app.animation_timer is None
        app.close()
        print("UI check passed: search, citations, sizing, transitions, reversal, reduced motion, close")
    else:
        root.mainloop()


if __name__ == "__main__":
    main()
