# Loft architecture

## Purpose and boundaries

Loft is a Windows desktop prototype for searching user-selected folders and answering with local models. Tkinter owns the UI; the engine owns indexing, retrieval, and inference. Runtime binaries, GGUF models, the SQLite library, and logs stay local and are excluded from Git.

## Components

| File | Responsibility |
| --- | --- |
| `loft_preview.py` | Desktop window, search and Chat modes, settings, folder management, background jobs, cancellation, and event-driven UI updates. |
| `loft_theme.py` | Windows palette and window appearance helpers. |
| `loft_engine.py` | Folder validation, file filtering, SQLite index, retrieval, model selection, and answer generation. |
| `loft_bench.py` | Local `llama-server` process wrapper, loopback HTTP helpers, and benchmark utilities. |
| `setup_runtime.py` | Pinned runtime and model downloads with checksum validation. |
| `engine_cli.py` | Headless index, ask, and folders commands. |
| `verify_engine.py` | Real-model verification run with a separate test index. |

## Data flow

```text
Selected folder -> file filters -> UTF-8 text -> line-based chunks
                -> Nomic embeddings -> SQLite files/chunks/FTS5/roots

Question -> lexical candidate gate -> embedding similarity + FTS ranking
         -> up to three current source passages -> Qwen answer model
         -> validated source IDs -> answer and source links

Chat tab -> selected Qwen answer model -> conversational reply
```

`Engine.index_folder` runs in a transaction. Unchanged file hashes skip re-embedding; removed files lose their index rows on refresh. The UI registers a selected root immediately so it appears in the folder list while indexing continues. If indexing fails, that root remains available for retry, while the previous indexed content is preserved. Folder changes require a manual refresh; there is no watcher yet.

The search path requires a lexical match before semantic ranking. This prevents weak similarity from being presented as evidence, but misses synonym-only matches. Before a passage is shown, Loft checks that its source file still matches the indexed digest. Questions with no supporting passage do not receive a file-grounded generated answer. The separate Chat tab uses the selected local answer model without file evidence and tells the model not to claim access to user files. Greetings in search mode also use that model.

## Models and process lifecycle

`LocalModels` reads `models/manifest.json`. Nomic Embed Text v1.5 is the fixed embedding model; the selected Qwen or imported GGUF handles answers and Chat. Model switches do not rebuild the index. An imported local GGUF is checked for its header and hashed locally; this does not verify its publisher provenance. Pinned downloads use publisher metadata and checksums.

Each inference stage starts a CPU-only `llama-server` child on a random loopback port with a per-process API key, waits for health, and stops that child afterward. The UI receives worker events through a queue. Startup percentages are stage milestones from server logs and health, not bytes loaded. Only one UI background job runs at a time; cancellation is cooperative and terminates Loft's own child process.

## Storage and limits

`data/library.sqlite3` stores paths, text chunks, vectors, and FTS data. `data/logs/` stores runtime logs. Preferences and model records also stay local. This is not encrypted storage or secure deletion. File filtering excludes links, common build/dependency output, binary content, likely secrets, and files over 1 MiB; secret detection is heuristic.

The vector scan is exact and suited to small libraries. The answer model can still produce claims that do not follow from a cited passage; validating source IDs does not validate every claim. PDF/OCR and Office readers, automatic watching, scalable vector indexing, packaging, and broader hardware evaluation remain future work.

## Verification

Run `python -m unittest` for deterministic engine/UI tests, `python loft_preview.py --check` for the demo UI check, and `python verify_engine.py` for an opt-in real-model run. Real inference requires loopback access to the local server.
