import io
import tempfile
import unittest
from pathlib import Path
from unittest.mock import patch
from urllib.error import HTTPError
from loft_bench import FakeEmbedding, Index, post


class IndexTests(unittest.TestCase):
    def setUp(self):
        test_root = Path(__file__).resolve().parent / "results"
        test_root.mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=test_root)
        self.path = Path(self.temp.name) / "index.sqlite3"
        self.index = Index(self.path, FakeEmbedding())
        self.docs = [dict(path="parking.txt", start=1, end=2, text="parking receipt expired yesterday"),
                     dict(path="login.py", start=8, end=9, text="login authenticate session")]

    def tearDown(self):
        self.index.db.close()
        self.temp.cleanup()

    def test_incremental_edit_delete_and_citations(self):
        self.assertEqual(self.index.sync(self.docs), 2)
        self.assertEqual(self.index.sync(self.docs), 0)
        hit = self.index.search("parking receipt")[0]
        self.assertEqual((hit["path"], hit["start"], hit["end"]), ("parking.txt", 1, 2))
        edited = [dict(self.docs[0], text="updated parking payment")]
        self.assertEqual(self.index.sync(edited), 1)
        self.assertEqual(self.index.db.execute("SELECT count(*) FROM words").fetchone()[0], 1)
        self.assertEqual(self.index.search("login")[0]["path"], "parking.txt")
        self.index.sync([])
        self.assertEqual(self.index.search("parking"), [])

    def test_model_change_rejected(self):
        embedder = FakeEmbedding()
        embedder.identity = "different-model"
        with self.assertRaisesRegex(ValueError, "identity"):
            Index(self.path, embedder)

    def test_failed_embedding_rolls_back(self):
        self.index.sync(self.docs)
        original = self.index.embedder.embed
        self.index.embedder.embed = lambda text: [1.0]
        with self.assertRaises(ValueError):
            self.index.sync([dict(self.docs[0], text="changed")])
        self.index.embedder.embed = original
        self.assertEqual(self.index.sync(self.docs), 0)

    def test_post_reports_http_error_body(self):
        error = HTTPError("http://127.0.0.1/x", 500, "err", None, io.BytesIO(b'{"error":"bad grammar"}'))
        with patch("loft_bench.HTTP.open", side_effect=error):
            with self.assertRaisesRegex(RuntimeError, "bad grammar"):
                post(9, "/v1/chat/completions", {"x": 1})


if __name__ == "__main__":
    unittest.main()
