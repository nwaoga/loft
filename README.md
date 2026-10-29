# Loft — local desktop assistant

Loft searches selected folders and generates short answers on the CPU with
source passages and line ranges. This is a working Windows prototype, not a
packaged release or a validated 8 GB minimum configuration.

## Run

The current workspace already has the virtual environment, runtime and models.

```powershell
.venv/Scripts/python loft_preview.py --expanded
```

For a fresh Windows x64 installation (Python 3.11+ with Tkinter):

```powershell
python -m venv .venv
.venv/Scripts/python -m pip install -r requirements.txt
.venv/Scripts/python setup_runtime.py
.venv/Scripts/python loft_preview.py --expanded
```

Setup downloads pinned llama.cpp CPU binaries, Qwen2.5-1.5B-Instruct Q4_K_M
(about 1.1 GB) and Nomic Embed Text v1.5 Q4_K_M. Downloads are checked against
publisher hashes. Revisions, sizes and hashes are saved in models/manifest.json.
The 0.5B answer model was tested but its citation selection was unreliable;
the 1.5B model is the provisional prototype default, not a final release choice.
Both model repositories declare Apache-2.0; llama.cpp uses MIT. Distribution
packaging must retain the applicable licenses and notices.

## Use your files

Settings lists the models used for answers and Chat. Download a listed Qwen
candidate, add a local GGUF file, then click Use to select it. Downloads show
progress and can be cancelled; installing does not activate a model automatically. Custom
models must support chat and structured output. Nomic embeddings stay separate,
so switching the answer model does not require rebuilding the index.

1. Open Folders, choose Add folder and select a directory.
2. Wait for indexing to finish; progress names the current stage and file.
3. Ask a file question in Everything, Documents or Code, or use Chat for general conversation.
4. Inspect Sources and click a citation to read the indexed passage.
5. Use Show original in folder to locate the file without executing it.

The selected folder appears immediately; the folder window shows indexing
stages and a final completion count. Refresh selected updates an included folder after edits, renames or deletions.
Remove selected removes its searchable records; original files are untouched.
Stop cancels work. The prior index is preserved if an indexing transaction fails
or is cancelled. A newly selected folder remains listed for retry even if its
first index attempt fails. There is no automatic folder watcher yet.

The larger answer window displays the full response. Source IDs are selected
by the model through a constrained response format; Loft validates that they
refer to retrieved passages. This does not prove that every generated claim
is supported. Inspect passages for dates, current status and ambiguous answers.

## Desktop controls

Click the notch to expand. Escape or the minus button collapses it; Ctrl+Q or
cross closes it. Ctrl+K focuses search while Loft has focus (not a global hotkey).
Expand and collapse use a 240 ms transition and respect Windows reduced motion.
Drag the title to move Loft. Settings includes glass/solid appearance, opacity,
keep-on-top and monitor placement. Preferences are stored under data/.
Taskbar colour provides a subtle tint and accent. macOS native behavior and
packaging have not been validated; the downloader currently targets Windows x64.

## Engine behavior and limits

- UTF-8 text, Markdown, CSV as plain text, and common source-code extensions.
- Multiple non-overlapping folders; nested and ancestor .gitignore rules.
- Skips hidden folders, dependencies/build output, links/junctions, binaries,
  files over 1 MiB and likely secrets. Secret detection is heuristic, not a guarantee.
- SHA-256 content checks avoid re-embedding unchanged files.
- Bounded line-based chunks preserve locations; language-aware parsing is pending.
- SQLite FTS5 plus exact cosine search combined with reciprocal rank fusion.
- Generation requires a keyword-matched passage. Embeddings rank those candidates;
  synonym-only matches are withheld pending retrieval-quality evaluation.
- Embedding identity, dimensions and index version protect against model mismatch.
- Changed/deleted source files are withheld from answers until refreshed.
- One background operation at a time, with cancellation; the UI stays responsive.
- Models load on demand and unload after each stage. Nothing uses a GPU or cloud
  inference. Runtime requests stay on loopback and use a per-process API key.
- Nomic receives its required search_document/search_query prefixes.
- Answers have a bounded 4096-token context and 240-token structured response;
  oversized prompts are rejected before generation.

PDF extraction/OCR, Word and spreadsheet readers, AST parsing, battery-aware
scheduling, automatic watching, installers and crash-recovery polish remain.
The exact vector scan is a small-library baseline; large libraries need a
scalable index. The small answer model can make mistakes or omit useful detail.

The index contains extracted text, paths and embeddings in data/library.sqlite3.
Runtime logs are under data/logs/. These files stay local and are not encrypted
by Loft. Removing records is not a secure erase of filesystem/database history.
Document content is treated as evidence, never as permission to run commands.

## Checks and measurement

```powershell
.venv/Scripts/python -m unittest -v
.venv/Scripts/python loft_preview.py --check
.venv/Scripts/python verify_engine.py
```

The first checks use deterministic test embeddings. The UI check uses demo data
without changing saved appearance preferences. verify_engine.py runs the real
models against this project in a separate index and writes a report to results/.
It measures initial/unchanged indexing, end-to-end query latency, first answer
text, sampled per-runtime peak RSS and CPU time. Runtime RSS is not combined
application/system peak memory; 100 ms sampling can miss short peaks. Runs may
use warm OS caches. These checks do not establish production answer quality.

```powershell
.venv/Scripts/python engine_cli.py index D:/path/to/folder
.venv/Scripts/python engine_cli.py ask "Where is login handled?" --scope Code
.venv/Scripts/python engine_cli.py folders
```

The original benchmark remains available as loft_bench.py, including --smoke.
Its synthetic timings do not establish AI performance. Use --demo on the desktop
preview for the original invented five-file library.

Measure representative larger corpora, idle/combined peak memory, battery use,
tail latency, and citation quality on modest Windows and Mac hardware before
choosing final model and desktop-framework defaults.

## References

- [llama.cpp server](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md)
- [Qwen answer model](https://huggingface.co/Qwen/Qwen2.5-1.5B-Instruct-GGUF)
- [Nomic embedding model](https://huggingface.co/nomic-ai/nomic-embed-text-v1.5-GGUF)

The handover suggested CerebroMind as an architectural reference. No code has
been copied from that project; its license and build remain unverified here.
