from contextlib import contextmanager
from pathlib import Path
import tempfile
import threading
import time
import unittest
import io
import json
from unittest.mock import patch

from loft_bench import ROOT, FakeEmbedding
from loft_engine import Engine, Cancelled, chunks, streamed_answer, LocalModels, unsupported_terms, format_dropped, read_indexed_text


class TestModels:
    identity = "test-v1"
    def __init__(self):
        self.calls = 0
    def embed(self, text, query=False):
        self.calls += 1
        return FakeEmbedding().embed(text)
    def chat(self, endpoint, message, cancel, history=None):
        self.calls += 1
        self.history = history
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
        self.write("tokenizer.py", "class Tokenizer:\n    pass\n")
        self.write("token.json", "{\"token\": \"skip-me\"}\n")
        self.index()
        with self.engine.connect() as db:
            names = {Path(row[0]).name for row in db.execute("SELECT path FROM files")}
        self.assertEqual(names, {"keep.txt", "yes.md", "tokenizer.py"})
        self.assertEqual(self.engine.root_count(self.engine.roots()[0]), 3)
    def test_index_names_why_files_were_left_out(self):
        self.write("notes.md", "parking receipt")
        self.write("token.json", "{}\n")
        self.write("build/app.js", "bundle()\n")
        self.write(".gitignore", "draft.md\n")
        self.write("draft.md", "do not index\n")
        self.write("private.md", "-----BEGIN PRIVATE KEY-----\n")
        stats = self.index()
        reasons = {item["name"]: item["reason"] for item in stats["dropped"]}
        self.assertEqual(reasons["token.json"], "likely secret")
        self.assertEqual(reasons["build"], "build folder")
        self.assertEqual(reasons["draft.md"], "gitignore")
        self.assertEqual(reasons["private.md"], "likely secret")
        self.assertNotIn("notes.md", reasons)
        self.assertGreaterEqual(stats["dropped_total"], 4)
        self.assertIn("token.json (likely secret)", format_dropped(stats))
        self.assertIn("build (build folder)", format_dropped(stats))
    def test_unsupported_terms_ignore_words_in_the_passage(self):
        gaps = unsupported_terms("Parking was paid. The registration is AB12.", ["Parking receipt paid eight pounds."])
        lowered = [gap.lower() for gap in gaps]
        self.assertIn("registration", lowered)
        self.assertIn("ab12", lowered)
        self.assertNotIn("paid", lowered)
        self.assertNotIn("parking", lowered)
        self.assertEqual(unsupported_terms("Paid 8 pounds.", ["Paid £8 pound."]), [])
        self.assertEqual(unsupported_terms("The animation takes 240 ms.", ["The animation takes 240 ms."]), [])
    def test_ask_flags_terms_missing_from_the_cited_passage(self):
        self.write("notes.md", "The animation takes 240 ms.")
        self.index()
        def answer(endpoint, question, hits, cancel, emit):
            text = "The registration number is AB12. [S1]"
            emit("token", text)
            return text
        self.models.answer = answer
        events = []
        result = self.engine.ask("animation", "Documents", self.cancel, lambda kind, value: events.append((kind, value)))
        gaps = [gap.lower() for gap in next(value for kind, value in events if kind == "support")]
        self.assertIn("registration", gaps)
        self.assertIn("ab12", gaps)
        self.assertNotIn("animation", gaps)
        self.assertEqual(result["unsupported"], next(value for kind, value in events if kind == "support"))
        self.models.answer = lambda endpoint, question, hits, cancel, emit: "The animation takes 240 ms. [S1]"
        kinds = []
        self.engine.ask("animation", "Documents", self.cancel, lambda kind, value: kinds.append(kind))
        self.assertNotIn("support", kinds)
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
    def test_code_chunks_break_before_the_next_definition_and_can_overlap(self):
        intro = "intro line stays with the header\n" * 40
        text = intro + "def login(username):\n    return authenticate(username)\n"
        passages = list(chunks(text, code=True))
        self.assertGreaterEqual(len(passages), 2)
        self.assertTrue(passages[-1][2].lstrip().startswith("def login"))
        self.assertNotIn("def login", passages[0][2])
        self.assertLessEqual(passages[-1][0], text[:text.index("def login")].count("\n") + 1)
        overlapped = list(chunks("alpha\nbeta\ngamma\ndelta\n", maximum=12, overlap_lines=1))
        self.assertGreaterEqual(len(overlapped), 2)
        self.assertTrue(overlapped[1][2].startswith(overlapped[0][2].splitlines(keepends=True)[-1]))
        self.assertLess(overlapped[1][0], overlapped[1][1])
    def test_pdf_text_is_searchable_and_secrets_stay_out(self):
        receipt = self._pdf("Parking receipt paid eight pounds")
        (self.folder / "receipt.pdf").write_bytes(receipt)
        (self.folder / "secret.pdf").write_bytes(self._pdf("api_key = AAAAAAAAAAAAAAAA"))
        (self.folder / "broken.pdf").write_bytes(b"%PDF-1.4\nnot a real pdf")
        (self.folder / "notes.md").write_bytes(b"hello\0world")
        text, reason = read_indexed_text(self.folder / "receipt.pdf", receipt + b"\0")
        self.assertIsNone(reason)
        self.assertIn("Page 1", text)
        self.assertIn("Parking receipt", text)
        self.assertEqual(read_indexed_text(self.folder / "notes.md", b"hello\0world")[1], "binary")
        stats = self.index()
        reasons = {item["name"]: item["reason"] for item in stats["dropped"]}
        self.assertEqual(reasons["secret.pdf"], "likely secret")
        self.assertEqual(reasons["broken.pdf"], "unreadable PDF")
        hits = self.engine.retrieve("parking receipt", "Documents", self.cancel, lambda *_: None)
        self.assertTrue(hits[0]["path"].endswith("receipt.pdf"))
        self.assertIn("Page 1", hits[0]["text"])
        self.assertIn("eight pounds", hits[0]["text"])

    def test_scanned_pdf_is_read_with_ocr(self):
        import io
        import fitz
        from PIL import Image, ImageDraw

        image = Image.new("RGB", (420, 80), "white")
        ImageDraw.Draw(image).text((12, 24), "Flight Lois itinerary", fill="black")
        buffer = io.BytesIO()
        image.save(buffer, format="PNG")
        document = fitz.open()
        page = document.new_page(width=420, height=80)
        page.insert_image(page.rect, stream=buffer.getvalue())
        raw = document.tobytes()
        document.close()
        text, reason = read_indexed_text(self.folder / "scan.pdf", raw)
        self.assertIsNone(reason)
        self.assertIn("Page 1", text)
        self.assertRegex(text.lower(), r"flight|lois|itinerary")

    def _pdf(self, sentence):
        return f"""%PDF-1.4
1 0 obj<</Type/Catalog/Pages 2 0 R>>endobj
2 0 obj<</Type/Pages/Count 1/Kids[3 0 R]>>endobj
3 0 obj<</Type/Page/Parent 2 0 R/MediaBox[0 0 300 144]/Contents 4 0 R/Resources<</Font<</F1 5 0 R>>>>>>endobj
4 0 obj<</Length {len(sentence) + 32}>>stream
BT /F1 18 Tf 40 80 Td ({sentence}) Tj ET
endstream
endobj
5 0 obj<</Type/Font/Subtype/Type1/BaseFont/Helvetica>>endobj
xref
0 6
0000000000 65535 f 
0000000009 00000 n 
0000000052 00000 n 
0000000101 00000 n 
0000000224 00000 n 
0000000350 00000 n 
trailer<</Size 6/Root 1 0 R>>
startxref
419
%%EOF
""".encode()
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

    def test_scope_keeps_document_hits_past_code_noise(self):
        for number in range(60):
            self.write(f"src/file{number}.py", "widget " * 20)
        self.write("notes.md", "widget appears once in the handbook")
        self.index()
        hits = self.engine.retrieve("widget", "Documents", self.cancel, lambda *_: None)
        self.assertEqual([Path(hit["path"]).name for hit in hits], ["notes.md"])

    def test_missing_file_during_read_does_not_abort(self):
        self.write("keep.md", "parking receipt")
        self.write("vanish.md", "temporary note")
        real_read = Path.read_bytes
        def flaky(path, *args, **kwargs):
            if path.name == "vanish.md":
                raise FileNotFoundError(path)
            return real_read(path, *args, **kwargs)
        with patch.object(Path, "read_bytes", flaky):
            stats = self.index()
        self.assertGreaterEqual(stats["skipped"], 1)
        with self.engine.connect() as db:
            names = {Path(row[0]).name for row in db.execute("SELECT path FROM files")}
        self.assertEqual(names, {"keep.md"})

    def test_unreadable_file_is_kept(self):
        self.write("keep.md", "parking receipt")
        locked = self.write("locked.md", "secret recipe")
        self.index()
        real_read = Path.read_bytes
        def flaky(path, *args, **kwargs):
            if path.name == "locked.md":
                raise PermissionError("denied")
            return real_read(path, *args, **kwargs)
        with patch.object(Path, "read_bytes", flaky):
            stats = self.index()
        self.assertEqual(stats["unreadable"], 1)
        with self.engine.connect() as db:
            text = db.execute("SELECT text FROM chunks WHERE path=?", (str(locked),)).fetchone()[0]
        self.assertIn("secret recipe", text)

    def test_bad_gitignore_names_the_file(self):
        (self.folder / ".gitignore").write_bytes(b"\xff\xfe")
        self.write("keep.md", "keep this")
        with self.assertRaisesRegex(RuntimeError, "Could not read"):
            self.index()

    def test_stale_warning_follows_sources_once(self):
        first = self.write("one.md", "alpha receipt")
        second = self.write("two.md", "alpha receipt extra")
        self.index()
        first.write_text("changed", encoding="utf-8")
        second.write_text("changed too", encoding="utf-8")
        events = []
        self.engine.ask("alpha", "Documents", self.cancel, lambda kind, value: events.append((kind, value)))
        kinds = [kind for kind, _ in events]
        self.assertLess(kinds.index("sources"), kinds.index("warning"))
        self.assertEqual(len([value for kind, value in events if kind == "warning"]), 1)
        self.assertIn("Refresh the folder", events[-1][1])

    def test_empty_library_explains_instead_of_searching(self):
        events = []
        result = self.engine.ask("parking", "All", self.cancel, lambda kind, value: events.append((kind, value)))
        self.assertIsNone(result["answer"])
        self.assertEqual(self.models.calls, 0)
        self.assertIn("No files are indexed", events[-1][1])

    def test_empty_scope_names_the_missing_kind(self):
        self.write("src/login.py", "def login():\n    return True\n")
        self.index()
        calls = self.models.calls
        events = []
        result = self.engine.ask("login", "Documents", self.cancel, lambda kind, value: events.append((kind, value)))
        self.assertIsNone(result["answer"])
        self.assertEqual(self.models.calls, calls)
        self.assertIn("No Documents files", events[-1][1])

    def test_long_question_does_not_crash(self):
        self.write("notes.md", "parking receipt")
        self.index()
        hits = self.engine.retrieve("x" * 5000, "All", self.cancel, lambda *_: None)
        self.assertEqual(hits, [])

    def test_spaced_greeting_skips_search(self):
        result = self.engine.ask("good  morning!", "All", self.cancel, lambda *_: None)
        self.assertEqual(result["answer"], "Hello from the model.")
        self.assertEqual(result["hits"], [])

    def test_absent_evidence_never_calls_generation(self):
        self.write("notes.md", "Expand and collapse take 240 ms.")
        self.index()
        calls = self.models.calls
        result = self.engine.ask("What is the registration number of my car?", "Documents", self.cancel, lambda *_: None)
        self.assertEqual(result["hits"], [])
        self.assertIsNone(result["answer"])
        self.assertEqual(self.models.calls, calls)


    def test_edit_marks_folder_stale_until_reindexed(self):
        file = self.write("parking.md", "parking receipt")
        self.index()
        root = self.engine.roots()[0]
        self.assertFalse(self.engine.root_is_stale(root))
        file.write_text("parking receipt\nedited\n", encoding="utf-8")
        self.assertTrue(self.engine.root_is_stale(root))
        self.index()
        self.assertFalse(self.engine.root_is_stale(root))
        file.unlink()
        self.assertTrue(self.engine.root_is_stale(root))
    def test_watcher_marks_creates_edits_and_ignores_build_noise(self):
        from loft_watch import watch_should_mark, _Handler
        root = self.folder
        self.assertTrue(watch_should_mark(root, root / "notes.md"))
        self.assertTrue(watch_should_mark(root, root / "receipt.pdf"))
        self.assertTrue(watch_should_mark(root, root / "src" / "login.py"))
        self.assertTrue(watch_should_mark(root, root / "src", is_directory=True))
        self.assertFalse(watch_should_mark(root, root / "node_modules" / "pkg" / "index.js"))
        self.assertFalse(watch_should_mark(root, root / "build" / "app.js"))
        self.assertFalse(watch_should_mark(root, root / ".git" / "config"))
        self.assertFalse(watch_should_mark(root, root / "photo.png"))
        marked = []
        handler = _Handler([str(root)], marked.append)
        handler.consider(root / "notes.md", False)
        handler.consider(root / "build" / "app.js", False)
        handler.consider(root / "gone.md", False)
        self.assertEqual(marked, [str(root), str(root)])
        self.write("notes.md", "parking receipt")
        self.index()
        self.engine.note_root_changed(self.engine.roots()[0])
        self.assertTrue(self.engine.root_is_stale(self.engine.roots()[0]))
        self.index()
        self.assertFalse(self.engine.root_is_stale(self.engine.roots()[0]))

    def test_create_modify_and_delete_mark_the_folder(self):
        from watchdog.events import FileCreatedEvent, FileDeletedEvent, FileModifiedEvent, FileSystemEventHandler
        from loft_watch import FolderWatcher
        root = str(self.folder)
        seen = []
        watcher = FolderWatcher(seen.append, delay=0.05)
        try:
            watcher._roots = [root]
            handler = watcher._event_handler(FileSystemEventHandler)
            note = self.folder / "notes.md"
            handler.on_created(FileCreatedEvent(str(note)))
            handler.on_modified(FileModifiedEvent(str(note)))
            handler.on_deleted(FileDeletedEvent(str(note)))
            time.sleep(0.2)
            self.assertEqual(seen, [root])
            seen.clear()
            handler.on_modified(FileModifiedEvent(str(self.folder / "photo.png")))
            handler.on_created(FileCreatedEvent(str(self.folder / "build" / "out.js")))
            time.sleep(0.2)
            self.assertEqual(seen, [])
        finally:
            watcher.close()

    def test_rapid_writes_debounce_to_one_notice(self):
        from loft_watch import FolderWatcher
        root = str(self.folder)
        seen = []
        watcher = FolderWatcher(seen.append, delay=0.25)
        try:
            for _ in range(6):
                watcher._queue(root)
                time.sleep(0.05)
            self.assertEqual(seen, [])
            time.sleep(0.45)
            self.assertEqual(seen, [root])
        finally:
            watcher.close()


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

    def test_empty_sources_do_not_claim_validation_failed(self):
        events = []
        with patch("loft_engine.post", side_effect=[io.BytesIO(b'{"tokens":[1]}'), self.response([], "The sources do not say.")]):
            answer = LocalModels().answer((1234, "test-key"), "duration?",
                [dict(citation="S1", path="note.md", start=1, end=1, text="240 ms")],
                threading.Event(), lambda kind, value: events.append((kind, value)))
        self.assertEqual(answer, "The sources do not say.")
        warning = next(value for kind, value in events if kind == "warning")
        self.assertNotIn("could not be validated", warning.lower())
        self.assertIn("No source was cited", warning)

    def test_citation_already_in_answer_is_not_duplicated(self):
        events = []
        with patch("loft_engine.post", side_effect=[io.BytesIO(b'{"tokens":[1]}'), self.response(["S1"], "See [S1] for the duration.")]):
            answer = LocalModels().answer((1234, "test-key"), "duration?",
                [dict(citation="S1", path="note.md", start=1, end=1, text="240 ms")],
                threading.Event(), lambda kind, value: events.append((kind, value)))
        self.assertEqual(answer, "See [S1] for the duration.")
        self.assertEqual(answer.count("[S1]"), 1)

    def test_download_stops_before_network_without_setup(self):
        with tempfile.TemporaryDirectory() as tmp:
            with patch("loft_engine.ROOT", Path(tmp)), patch("setup_runtime.model") as fetch:
                models = LocalModels()
                with self.assertRaisesRegex(RuntimeError, "not installed"):
                    models.download_answer(LocalModels.CATALOG[0], threading.Event(), lambda *_: None)
                fetch.assert_not_called()

    def test_selected_answer_ignores_a_missing_default(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            models_dir = root / "models"
            models_dir.mkdir()
            (models_dir / "other.gguf").write_bytes(b"GGUF" + b"o" * 16)
            manifest = {
                "runtime": {"server": "runtime/llama-server"},
                "models": [{"file": "missing.gguf"}, {"file": "embed.gguf"}],
                "answer_models": [{"file": "other.gguf", "title": "Other"}],
                "selected_answer": "missing.gguf",
            }
            (models_dir / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            with patch("loft_engine.ROOT", root):
                models = LocalModels()
                self.assertEqual(models.selected_answer()["file"], "other.gguf")

    def test_import_keeps_the_current_model_and_can_select_the_only_file(self):
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            models_dir = root / "models"
            models_dir.mkdir()
            (models_dir / "qwen.gguf").write_bytes(b"GGUF" + b"q" * 32)
            manifest = {
                "runtime": {"server": "runtime/llama-server"},
                "models": [{"file": "qwen.gguf", "title": "Qwen"}, {"file": "embed.gguf"}],
                "selected_answer": "qwen.gguf",
            }
            (models_dir / "manifest.json").write_text(json.dumps(manifest), encoding="utf-8")
            custom = root / "custom.gguf"
            custom.write_bytes(b"GGUF" + b"c" * 32)
            with patch("loft_engine.ROOT", root):
                models = LocalModels()
                models.import_answer(custom, threading.Event(), lambda *_: None)
                self.assertEqual(models.manifest["selected_answer"], "qwen.gguf")
                self.assertIn(str(custom.resolve()), {item["file"] for item in models.answer_models()})
                models.manifest["selected_answer"] = "missing.gguf"
                (models_dir / "qwen.gguf").unlink()
                models.manifest["answer_models"] = [item for item in models.manifest["answer_models"] if item["file"] != "qwen.gguf"]
                models.register_answer({"file": str(custom.resolve()), "title": "custom", "repo": "Local file",
                                         "bytes": custom.stat().st_size, "sha256": "abc", "license": "Check model licence"})
                self.assertEqual(models.selected_answer()["file"], str(custom.resolve()))

    def test_chat_sends_the_recent_transcript(self):
        seen = {}
        def fake_post(port, route, body, api_key=None):
            seen["messages"] = body["messages"]
            return io.BytesIO(b'{"choices":[{"message":{"content":"Second reply"}}]}')
        history = [{"role": "user", "content": "First question"},
                   {"role": "assistant", "content": "First reply"}]
        with patch("loft_engine.post", fake_post):
            answer = LocalModels().chat((9, "key"), "Second question", threading.Event(), history)
        self.assertEqual(answer, "Second reply")
        roles = [message["role"] for message in seen["messages"]]
        self.assertEqual(roles, ["system", "user", "assistant", "user"])
        self.assertEqual(seen["messages"][1]["content"], "First question")
        self.assertEqual(seen["messages"][-1]["content"], "Second question")
        long = [{"role": "user", "content": f"turn {i}"} for i in range(8)]
        with patch("loft_engine.post", fake_post):
            LocalModels().chat((9, "key"), "Now", threading.Event(), long)
        self.assertEqual([message["content"] for message in seen["messages"][1:-1]], [f"turn {i}" for i in range(2, 8)])

    def test_invalid_model_source_rejected(self):
        with patch("loft_engine.post", side_effect=[io.BytesIO(b'{"tokens":[1]}'), self.response(["S99"])]):
            with self.assertRaisesRegex(RuntimeError, "invalid source"):
                LocalModels().answer((1234, "test-key"), "duration?",
                    [dict(citation="S1", path="note.md", start=1, end=1, text="240 ms")],
                    threading.Event(), lambda *_: None)


if __name__ == "__main__":
    unittest.main()
