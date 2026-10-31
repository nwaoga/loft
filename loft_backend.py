"""Swap local llama.cpp for another answer or embedding backend.

CerebroMind splits this into IEmbeddingService.CreateEmbeddingAsync and
IAIService.CreateAnswerAsync / CreateCustomChatCompletionAsync, selected by
config. Loft keeps the same split, with local llama.cpp as the default:

- session("embedding") yields an object with identity and embed(text, query=False).
- session("answer") yields a handle passed to answer() and chat().
- Engine.ask still validates citations and unsupported words after answer().

The desktop window constructs Engine() with no backend, so it keeps LocalModels.
"""
from contextlib import contextmanager
from typing import Protocol

from loft_bench import FakeEmbedding


class EmbeddingModel(Protocol):
    identity: str
    def embed(self, text: str, query: bool = False) -> list[float]: ...


class InferenceBackend(Protocol):
    def session(self, kind: str, cancel, emit=None): ...
    def answer(self, endpoint, question, hits, cancel, emit) -> str: ...
    def chat(self, endpoint, message, cancel, history=None) -> str: ...


def default_backend():
    """Local llama.cpp for answers and Nomic embeddings."""
    from loft_engine import LocalModels
    return LocalModels()


class FixedBackend:
    """In-process answers and hash embeddings. The desktop UI does not select this."""

    identity = "fixed-backend-v1"

    def __init__(self, answer_text="The note is on file. [S1]", chat_text="Fixed chat."):
        self.answer_text = answer_text
        self.chat_text = chat_text
        self.history = None
        self._embed = FakeEmbedding()

    def embed(self, text, query=False):
        return self._embed.embed(text)

    def answer(self, endpoint, question, hits, cancel, emit):
        emit("token", self.answer_text)
        return self.answer_text

    def chat(self, endpoint, message, cancel, history=None):
        self.history = list(history or [])
        return self.chat_text

    @contextmanager
    def session(self, kind, cancel, emit=None):
        yield self
