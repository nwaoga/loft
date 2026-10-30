"""Exercise the worker-to-Tk path without loading a model or saved preferences."""
from pathlib import Path
import tempfile
import threading
import tkinter as tk
import unittest
from unittest.mock import patch

from loft_preview import LoftPreview


class StubEngine:
    def __init__(self):
        self.roots_list = []
        self.indexed = []
        self.removed = []
    def count(self, scope="All"):
        return 1
    def roots(self):
        return list(self.roots_list)
    def root_count(self, root):
        return 0
    def normalize_root(self, folder):
        path = Path(folder).resolve()
        if not path.is_dir():
            raise ValueError("Choose a folder")
        for existing in self.roots_list:
            other = Path(existing)
            if path != other and (path.is_relative_to(other) or other.is_relative_to(path)):
                raise ValueError("That folder overlaps an included folder")
        return str(path)
    def add_root(self, folder):
        folder = self.normalize_root(folder)
        if folder not in self.roots_list:
            self.roots_list.append(folder)
        return folder
    def remove(self, root):
        self.roots_list = [item for item in self.roots_list if item != root]
        self.removed.append(root)
    def index_folder(self, folder, cancel, emit):
        self.indexed.append(folder)
        emit("status", "Reading your files…")
        return {"indexed": 1, "unchanged": 0, "deleted": 0, "skipped": 0, "unreadable": 0}
    def ask(self, question, scope, cancel, emit):
        emit("status", "Searching your files…")
        emit("sources", [dict(citation="S1", path="notes.md", start=1, end=1, text="240 ms")])
        emit("token", "The animation takes 240 ms.")
        emit("token", " [S1]")
        emit("status", "Local answer · check the supporting sources")
        return {"answer": "The animation takes 240 ms. [S1]"}
    def chat_question(self, question, cancel, emit):
        emit("chat", "Hello from chat.")
        emit("status", "Local chat · not based on your files")
        return {"answer": "Hello from chat."}


class UIWorkerTests(unittest.TestCase):
    def setUp(self):
        self.root = tk.Tk()
        with patch("loft_engine.Engine", return_value=StubEngine()):
            self.app = LoftPreview(self.root, use_settings=False)
        self.app.motion_ready = False
        self.app.expand()
    def tearDown(self):
        self.app.close()
    def wait_idle(self):
        done = tk.BooleanVar(self.root, False)
        def check():
            if not self.app.busy:
                done.set(True)
            else:
                self.root.after(20, check)
        self.root.after(20, check)
        timeout = self.root.after(3000, lambda: done.set(True))
        self.root.wait_variable(done)
        self.root.after_cancel(timeout)
        self.assertFalse(self.app.busy)
    def test_stream_sources_and_full_answer(self):
        self.app.ask("How long?")
        self.wait_idle()
        self.assertEqual(self.app.answer_buffer, "The animation takes 240 ms. [S1]")
        self.assertEqual(self.app.last_hits[0]["citation"], "S1")
        self.assertTrue(self.app.sources_open)
        self.assertTrue(self.app.sources_detail.winfo_ismapped())
        self.assertEqual(self.app.entry.cget("state"), "normal")
        self.assertIsNone(self.app.stop_button)
        self.app.show_answer()
        self.assertIn("[S1]", self.app.full_answer.get("1.0", "end"))
    def test_settings_do_not_destroy_an_active_answer(self):
        release = threading.Event()
        self.app.start_job(lambda cancel, emit: release.wait(1))
        children = self.app.results.winfo_children()
        self.app.show_settings()
        self.assertEqual(children, self.app.results.winfo_children())
        release.set()
        self.wait_idle()

    def test_search_while_busy_explains_why(self):
        release = threading.Event()
        self.app.start_job(lambda cancel, emit: release.wait(2), kind="ask")
        self.app.question.set("hello")
        self.app.search()
        self.assertIn("running", self.app.phase_text.lower())
        release.set()
        self.wait_idle()

    def test_scope_stays_put_while_busy_and_chat_does_not_auto_send(self):
        release = threading.Event()
        self.app.start_job(lambda cancel, emit: release.wait(2), kind="ask")
        self.app.question.set("Where is login?")
        self.app.select_scope("Chat")
        self.assertEqual(self.app.scope, "All")
        release.set()
        self.wait_idle()
        calls = []
        self.app.engine.chat_question = lambda *args: calls.append(args)
        self.app.select_scope("Chat")
        self.root.update()
        self.assertEqual(self.app.scope, "Chat")
        self.assertIn("Press Enter", self.app.status.cget("text"))
        self.assertEqual(calls, [])
        self.assertFalse(self.app.busy)

    def test_empty_search_and_worker_error_are_visible(self):
        def ask(question, scope, cancel, emit):
            emit("sources", [])
            emit("status", "No keyword match in your indexed folders.")
            return {"hits": [], "answer": None}
        self.app.engine.ask = ask
        self.app.ask("missing term")
        self.wait_idle()
        notes = [widget.cget("text") for widget in self.app.results.winfo_children() if isinstance(widget, tk.Label)]
        self.assertTrue(any("keyword match" in text for text in notes))
        def fail(question, scope, cancel, emit):
            raise RuntimeError("Model server failed")
        self.app.engine.ask = fail
        self.app.ask("hello again")
        self.wait_idle()
        self.assertIn("Model server failed", self.app.status.cget("text"))
        self.assertEqual(self.app.entry.cget("state"), "normal")
        notes = [widget.cget("text") for widget in self.app.results.winfo_children() if isinstance(widget, tk.Label)]
        self.assertTrue(any("Model server failed" in text for text in notes))

    def test_index_summary_includes_skipped_files(self):
        self.app.manage_library()
        def operation(cancel, emit):
            return {"indexed": 0, "unchanged": 0, "deleted": 0, "skipped": 3, "unreadable": 1}
        self.assertTrue(self.app.start_job(operation, kind="index"))
        self.wait_idle()
        self.assertIn("3 skipped", self.app.library_status.cget("text"))
        self.assertIn("1 unreadable", self.app.status.cget("text"))
    def test_index_names_a_sample_of_dropped_files(self):
        self.app.manage_library()
        def operation(cancel, emit):
            return {"indexed": 1, "unchanged": 0, "deleted": 0, "skipped": 2,
                    "dropped": [{"name": "token.json", "reason": "likely secret"},
                                {"name": "build", "reason": "build folder"}],
                    "dropped_total": 4}
        self.assertTrue(self.app.start_job(operation, kind="index"))
        self.wait_idle()
        status = self.app.library_status.cget("text")
        self.assertIn("token.json (likely secret)", status)
        self.assertIn("build (build folder)", status)
        self.assertIn("and 2 more", status)
        notes = [child.cget("text") for child in self.app.results.winfo_children() if isinstance(child, tk.Label)]
        self.assertTrue(any("token.json (likely secret)" in text for text in notes))
    def test_unsupported_answer_terms_are_flagged(self):
        def ask(question, scope, cancel, emit):
            emit("sources", [dict(citation="S1", path="notes.md", start=1, end=1, text="The animation takes 240 ms.")])
            emit("token", "The registration number is AB12. [S1]")
            emit("support", ["registration", "AB12"])
            emit("status", "Local answer · some words are not in the cited passage")
            return {"answer": "The registration number is AB12. [S1]"}
        self.app.engine.ask = ask
        self.app.ask("animation")
        self.wait_idle()
        self.assertEqual(self.app.support_line(), "Not in the cited passage: registration, AB12")
        caption = self.app.answer_card.winfo_children()[0].cget("text")
        self.assertIn("not fully supported", caption)
        box = self.app.answer_text
        self.assertIsInstance(box, tk.Text)
        self.assertTrue(box.tag_nextrange("gap", "1.0"))
        self.app.show_answer()
        self.root.update()
        self.assertIn("not fully supported", self.app.answer_window.title())
        self.assertTrue(self.app.full_answer.tag_nextrange("gap", "1.0"))
        self.assertIn("registration", self.app.answer_gap_note.cget("text"))

    def test_refresh_without_selection_explains(self):
        self.app.manage_library()
        self.app.refresh_folder()
        self.assertIn("Select a folder", self.app.library_status.cget("text"))

    def test_queued_folders_wait_without_cancelling(self):
        release = threading.Event()
        entered = threading.Event()
        def operation(cancel, emit):
            entered.set()
            release.wait(2)
            if cancel.is_set():
                raise RuntimeError("cancelled")
            return {"ok": True}
        self.assertTrue(self.app.start_job(operation, kind="index"))
        self.assertTrue(entered.wait(1))
        with tempfile.TemporaryDirectory() as tmp:
            first = Path(tmp) / "one"
            second = Path(tmp) / "two"
            first.mkdir()
            second.mkdir()
            self.app.include_folder(first)
            self.app.include_folder(second)
            self.assertFalse(self.app.cancel_work.is_set())
            self.assertEqual(len(self.app.pending_folders), 2)
            release.set()
            self.wait_idle()
            self.assertEqual(self.app.engine.indexed, [str(first.resolve()), str(second.resolve())])

    def test_stop_keeps_queued_folders_for_refresh(self):
        release = threading.Event()
        entered = threading.Event()
        def operation(cancel, emit):
            entered.set()
            release.wait(2)
            return {"ok": True}
        self.app.start_job(operation, kind="ask")
        self.assertTrue(entered.wait(1))
        with tempfile.TemporaryDirectory() as tmp:
            folder = Path(tmp) / "docs"
            folder.mkdir()
            self.app.include_folder(folder)
            self.app.cancel_work.set()
            release.set()
            self.wait_idle()
            self.assertEqual(self.app.engine.indexed, [])
            self.assertIn(str(folder.resolve()), self.app.pending_folders)

    def test_escape_stops_a_running_job(self):
        release = threading.Event()
        entered = threading.Event()
        def operation(cancel, emit):
            entered.set()
            release.wait(2)
            if cancel.is_set():
                raise RuntimeError("stopped internally")
            return None
        self.app.start_job(operation, kind="ask")
        self.assertTrue(entered.wait(1))
        self.app.on_escape()
        self.assertTrue(self.app.cancel_work.is_set())
        self.assertTrue(self.app.expanded)
        release.set()
        self.wait_idle()
        self.assertFalse(self.app.busy)


if __name__ == "__main__":
    unittest.main()
