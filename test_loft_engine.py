from contextlib import contextmanager
from pathlib import Path
import tempfile
import threading
import unittest
import io
import json
from unittest.mock import patch

from loft_bench import ROOT, FakeEmbedding
from loft_engine import Engine, Cancelled, chunks, streamed_answer, LocalModels


class TestModels:
    identity = "test-v1"
    def __init__(self):
        self.calls = 0
    def embed(self, text, query=False):
        self.calls += 1
        return FakeEmbedding().embed(text)
    def chat(self, endpoint, message, cancel):
        self.calls += 1
        return "Hello from the model."
    @contextmanager
    def session(self, kind, cancel, emit=None):
        yield self


class EngineTests(unittest.TestCase):
    def setUp(self):
        (ROOT / "results").mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=ROOT, prefix="engine-test-")
        self.base = Path(self.temp.name)
        self.folder = self.base / "documents"
        self.folder.mkdir()
        self.models = TestModels()
        self.engine = Engine(self.base / "library.sqlite3", self.models)
        self.cancel = threading.Event()
    def tearDown(self):
        self.temp.cleanup()
    def write(self, path, content):
        file = self.folder / path
        file.parent.mkdir(parents=True, exist_ok=True)
        file.write_text(content, encoding="utf-8")
        return file
    def index(self):
        return self.engine.index_folder(self.folder, self.cancel)
    def test_greeting_uses_answer_model_without_searching(self):
        events = []
        result = self.engine.ask("Hi!", "All", self.cancel, lambda kind, value: events.append((kind, value)))
        self.assertEqual(result["answer"], "Hello from the model.")
        self.assertIn("chat", [kind for kind, _ in events])
        self.assertEqual(self.models.calls, 1)
    def test_chat_mode_uses_model_without_file_search(self):
        events = []
        result = self.engine.chat_question("What can you help with?", self.cancel,
                                           lambda kind, value: events.append((kind, value)))
        self.assertEqual(result["answer"], "Hello from the model.")
        self.assertEqual(result["hits"], [])
        self.assertIn("chat", [kind for kind, _ in events])
    def test_added_folder_is_listed_before_indexing(self):
        path = self.engine.add_root(self.folder)
        self.assertEqual(self.engine.roots(), [path])
        self.assertEqual(self.engine.count(), 0)
        self.assertEqual(self.index()["indexed"], 0)
        self.assertEqual(self.engine.roots(), [path])
    def test_real_files_persist_incremental_rename_delete(self):
        file = self.write("parking.md", "Parking receipt expired on 12 March 2025.\nPaid eight pounds.")
        self.assertEqual(self.index()["indexed"], 1)
        calls = self.models.calls
        self.assertEqual(self.index()["unchanged"], 1)
        self.assertEqual(self.models.calls, calls)
        reopened = Engine(self.engine.database, self.models)
        hits = reopened.retrieve("parking receipt", "Documents", self.cancel, lambda *_: None)
        self.assertEqual(hits[0]["path"], str(file))
        self.assertEqual((hits[0]["start"], hits[0]["end"]), (1, 2))
        file.rename(self.folder / "receipt.md")
        stats = self.index()
        self.assertEqual((stats["indexed"], stats["deleted"]), (1, 1))
        (self.folder / "receipt.md").unlink()
        self.assertEqual(self.index()["deleted"], 1)
        self.assertEqual(self.engine.count(), 0)
    def test_ignores_nested_negation_binary_secrets_and_builds(self):
        self.write(".gitignore", "*.txt\n!keep.txt\nignored/\n")
        self.write("skip.txt", "skip")
        self.write("keep.txt", "keep")
        self.write("nested/.gitignore", "*.md\n!yes.md\n")
        self.write("nested/no.md", "skip")
        self.write("nested/yes.md", "keep")
        self.write("ignored/a.md", "skip")
        self.write("node_modules/a.js", "skip")
        self.write(".env", "password=secret")
        self.write("secrets.json", '{"token": "skip"}')
        self.write("binary.txt", "hello\0world")
        self.write("private.md", "-----BEGIN PRIVATE KEY-----")
        self.index()
        with self.engine.connect() as db:
            names = {Path(row[0]).name for row in db.execute("SELECT path FROM files")}
        self.assertEqual(names, {"keep.txt", "yes.md"})
    def test_cancel_rolls_back_and_model_change_rejected(self):
        self.write("first.md", "old content")
        self.index()
        self.write("first.md", "new content")
        def cancel_on_progress(kind, value):
            if value.startswith("Indexing"):
                self.cancel.set()
        with self.assertRaises(Cancelled):
            self.engine.index_folder(self.folder, self.cancel, cancel_on_progress)
        with self.engine.connect() as db:
            self.assertEqual(db.execute("SELECT text FROM chunks").fetchone()[0], "old content")
        self.cancel.clear()
        self.models.identity = "test-v2"
        with self.assertRaisesRegex(ValueError, "model changed"):
            self.index()
    def test_stale_sources_excluded_and_removal_purges(self):
        file = self.write("parking.md", "parking receipt")
        self.index()
        file.write_text("changed", encoding="utf-8")
        hits = self.engine.retrieve("parking", "Documents", self.cancel, lambda *_: None)
        self.assertEqual(hits, [])
        self.engine.remove(str(self.folder))
        self.assertEqual(self.engine.roots(), [])
        with self.engine.connect() as db:
            self.assertEqual(db.execute("SELECT count(*) FROM words").fetchone()[0], 0)
    def test_chunk_bounds_and_lines(self):
        text = "a"*2200 + "\nsecond\nthird"
        passages = list(chunks(text))
        self.assertEqual("".join(p[2] for p in passages), text)
        self.assertTrue(all(len(p[2]) <= 1000 for p in passages))
        self.assertEqual(passages[-1][1], 3)
    def test_search_names_each_wait(self):
        self.write("notes.md", "Run python loft_preview.py --check to exercise the preview.")
        self.index()
        phases = []
        self.engine.retrieve("preview check", "Documents", self.cancel,
                             lambda kind, value: phases.append(value) if kind == "status" else None)
        self.assertEqual(phases, ["Loading the search model…", "Searching your files…"])
    def test_overlapping_roots_rejected(self):
        self.write("sub/a.md", "hello")
        self.index()
        with self.assertRaisesRegex(ValueError, "overlaps"):
            self.engine.index_folder(self.folder / "sub", self.cancel)

    def test_absent_evidence_never_calls_generation(self):
        self.write("notes.md", "Expand and collapse take 240 ms.")
        self.index()
        calls = self.models.calls
        result = self.engine.ask("What is the registration number of my car?", "Documents", self.cancel, lambda *_: None)
        self.assertEqual(result["hits"], [])
        self.assertIsNone(result["answer"])
        self.assertEqual(self.models.calls, calls)


class AnswerTests(unittest.TestCase):
    def test_partial_json_escape_decoding(self):
        answer = 'Line one\n"quoted" \\ folder and café'
        raw = json.dumps({"answer": answer, "sources": ["S1"]})
        previous = ""
        for i in range(len(raw)+1):
            decoded = streamed_answer(raw[:i])
            self.assertTrue(decoded.startswith(previous))
            previous = decoded
        self.assertEqual(previous, answer)

    def response(self, selected, answer="The duration is 240 ms."):
        raw = json.dumps({"answer": answer, "sources": selected})
        stream = b"".join(b"data: " + json.dumps({"choices": [{"delta": {"content": token}}]}).encode() + b"\n\n" for token in raw)
        return io.BytesIO(stream + b"data: [DONE]\n\n")

    def test_streamed_text_uses_model_selected_citations_and_auth(self):
        events = []
        with patch("loft_engine.post", side_effect=[io.BytesIO(b'{"tokens":[1]}'), self.response(["S1"])]) as request:
            answer = LocalModels().answer((1234, "test-key"), "duration?",
                [dict(citation="S1", path="note.md", start=1, end=1, text="240 ms")],
                threading.Event(), lambda kind, value: events.append((kind, value)))
        self.assertEqual(answer, "The duration is 240 ms. [S1]")
        self.assertEqual("".join(v for k,v in events if k == "token"), answer)
        self.assertTrue(all(c.args[3] == "test-key" for c in request.call_args_list))

    def test_invalid_model_source_rejected(self):
        with patch("loft_engine.post", side_effect=[io.BytesIO(b'{"tokens":[1]}'), self.response(["S99"])]):
            with self.assertRaisesRegex(RuntimeError, "invalid source"):
                LocalModels().answer((1234, "test-key"), "duration?",
                    [dict(citation="S1", path="note.md", start=1, end=1, text="240 ms")],
                    threading.Event(), lambda *_: None)


if __name__ == "__main__":
    unittest.main()
