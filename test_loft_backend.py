"""A swapped backend still goes through Loft's citation checks."""
from pathlib import Path
import tempfile
import threading
import unittest

from loft_backend import FixedBackend, default_backend
from loft_engine import Engine, LocalModels


class BackendTests(unittest.TestCase):
    def test_default_backend_is_local_llama(self):
        with tempfile.TemporaryDirectory() as tmp:
            engine = Engine(Path(tmp) / "library.sqlite3")
        self.assertIsInstance(engine.backend, LocalModels)
        self.assertIs(engine.models, engine.backend)
        self.assertIsInstance(default_backend(), LocalModels)

    def test_swapped_backend_keeps_citation_and_support_checks(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            folder = root / "docs"
            folder.mkdir()
            (folder / "notes.md").write_text("Parking receipt paid eight pounds.\n", encoding="utf-8")
            settings = object()
            backend = FixedBackend("The registration is AB12. [S1]")
            engine = Engine(root / "library.sqlite3", models=settings, backend=backend)
            self.assertIs(engine.models, settings)
            self.assertIs(engine.backend, backend)
            cancel = threading.Event()
            engine.index_folder(folder, cancel)
            events = []
            result = engine.ask("parking", "Documents", cancel, lambda kind, value: events.append((kind, value)))
        gaps = [gap.lower() for gap in next(value for kind, value in events if kind == "support")]
        self.assertIn("registration", gaps)
        self.assertIn("ab12", gaps)
        self.assertNotIn("parking", gaps)
        self.assertIn("[S1]", result["answer"])
        self.assertTrue(result["hits"][0]["path"].endswith("notes.md"))

    def test_swapped_chat_receives_recent_turns(self):
        backend = FixedBackend(chat_text="Fixed chat.")
        with tempfile.TemporaryDirectory() as tmp:
            engine = Engine(Path(tmp) / "library.sqlite3", backend=backend)
            history = [{"role": "user", "content": "First"}, {"role": "assistant", "content": "Reply"}]
            answer = engine.chat_question("Second", threading.Event(), lambda *_: None, history)
        self.assertEqual(answer["answer"], "Fixed chat.")
        self.assertEqual(backend.history[0]["content"], "First")
        self.assertEqual(backend.history[1]["role"], "assistant")


if __name__ == "__main__":
    unittest.main()
