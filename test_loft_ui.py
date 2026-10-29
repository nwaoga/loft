"""Exercise the worker-to-Tk path without loading a model or saved preferences."""
import threading
import tkinter as tk
import unittest
from unittest.mock import patch

from loft_preview import LoftPreview


class StubEngine:
    def count(self):
        return 1
    def roots(self):
        return []
    def ask(self, question, scope, cancel, emit):
        emit("status", "Searching your files…")
        emit("sources", [dict(citation="S1", path="notes.md", start=1, end=1, text="240 ms")])
        emit("token", "The animation takes 240 ms.")
        emit("token", " [S1]")
        emit("status", "Local answer · check the supporting sources")
        return {"answer": "The animation takes 240 ms. [S1]"}


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


if __name__ == "__main__":
    unittest.main()
