# Loft V2 architecture

Loft answers questions about the files in folders you choose, entirely on your own computer.
This document describes how it is put together: the processes, the code, the data, and the
reasons behind the main decisions. For setup and usage see [README.md](README.md).

## The shape of it

One desktop app, three kinds of process, all on one machine and talking over `127.0.0.1`:

```
┌──────────────────────────── Loft (one process) ─────────────────────────────┐
│                                                                              │
│  Notch UI  (Svelte 5, in the system webview)                                 │
│    pill ⇄ panel · Ask · Folders · Settings · Chat & Answers options          │
│                     │  ▲                                                     │
│        commands     │  │  events                                             │
│        (invoke)     ▼  │  (emit)                                             │
│  Rust core  (Tauri 2)                                                        │
│    runtime   start, watch and stop the model servers                         │
│    models    catalog, downloads, checksums                                   │
│    indexer   walk → parse (+OCR) → chunk → embed → store                     │
│    jobs      index queue, file watching, folder commands                     │
│    store     SQLite + sqlite-vec: files, chunks, vectors                     │
│    rag       retrieve → prompt → streamed answer                             │
│    vision    optional picture descriptions                                   │
│                                                                              │
└───────┬───────────────────────────┬───────────────────────────┬─────────────┘
        │ HTTP                      │ HTTP                      │ HTTP
┌───────▼─────────┐        ┌────────▼────────┐        ┌─────────▼───────────┐
│ llama-server    │        │ llama-server    │        │ llama-server        │
│ chat model      │        │ embedding model │        │ picture model       │
│ always running  │        │ always running  │        │ only while indexing │
└─────────────────┘        └─────────────────┘        └─────────────────────┘
```

Nothing leaves the computer except two kinds of download that the user starts: model files
from Hugging Face, and the llama.cpp builds fetched by `npm run fetch-llama` at setup.

**Why separate servers.** Embedding models pool their output and do not reliably serve chat
completions from the same process, so chat and embeddings each get a `llama-server`. Using
llama.cpp's own server (not a linked library) keeps Loft free of C++ build steps and lets a
GPU build be swapped for a CPU build at run time.

## Code map

### Frontend: `src/`

| File | Role |
|---|---|
| `main.ts` | Applies the saved theme and placement, then mounts the app. |
| `App.svelte` | The notch shell: pill, panel, tab strip, status bar; top and right-edge layouts. |
| `lib/components/Ask.svelte` | The conversation: question box, streamed answers, sources drawer, context meter. |
| `lib/components/ChatOptions.svelte` | The "Chat & Answers" options sheet. |
| `lib/components/Folders.svelte` | Folder list with status, progress and actions. |
| `lib/components/Settings.svelte` | Theme, placement, display, models, launch at login, quit. |
| `lib/components/Models.svelte` | Chat, embedding and picture models: download, select. |
| `lib/api.ts` | Every call to the backend, with mock data when run in a plain browser. |
| `lib/state.svelte.ts` | Panel state: expanded, current tab, answer in progress. |
| `lib/chat-options.svelte.ts` | Chat options and their persistence. |
| `lib/theme.svelte.ts`, `lib/placement.svelte.ts` | Theme and notch placement. |
| `lib/types.ts` | Data shapes shared with the Rust side. |
| `app.css` | Theme variables and shared styles. |

The three tab views are always mounted and shown or hidden, so an answer keeps streaming
while another tab is open. `api.ts` detects whether it is inside Tauri; outside it serves
mock data, which is how the UI is developed and previewed without building the backend.

### Backend: `src-tauri/src/`

| Module | Role |
|---|---|
| `lib.rs` | App state, all Tauri commands, window sizing and docking, tray, hotkey, server supervision. |
| `runtime.rs` | Finds the llama.cpp build, chooses GPU or CPU and which GPU, spawns and stops servers. |
| `models.rs` | Downloads a model file with resume, checks the GGUF header and the SHA-256. |
| `parse.rs` | Turns a file into text sections: text and code, PDF, docx, pptx, spreadsheets, pictures. |
| `ocr.rs` | Text recognition for pictures and scanned PDF pages (Windows' built-in engine). |
| `chunk.rs` | Splits sections into overlapping chunks on paragraph boundaries. |
| `embed.rs` | Client for the embedding server. |
| `indexer.rs` | Brings one folder's index up to date. Free of Tauri types, so it is tested directly. |
| `jobs.rs` | Index queue, file watchers with debouncing, folder commands, progress events. |
| `store.rs` | The SQLite database: folders, files, chunks, vectors, search. |
| `rag.rs` | Retrieval, prompts, chat options, follow-up rewriting, answer streaming, usage stats. |
| `vision.rs` | Starts the picture model on demand and writes short picture descriptions. |
| `opener.rs` | Opens a cited file safely, a PDF at its page where possible. |

## Data

| What | Where |
|---|---|
| Models (`*.gguf`) | `<app data>/models/` |
| Index (`loft.db`) | `<app data>/` |
| Model selection, placement, display, picture model (`selection.json`) | `<app data>/` |
| Theme, chat options, saved conversation | the webview's local storage |
| llama.cpp builds | `binaries/` beside the app (bundled as resources) |

`<app data>` is the OS app-data folder for the identifier `com.loft.desktop`
(`%APPDATA%\com.loft.desktop` on Windows).

Settings the backend needs before the UI exists (where to put the window, which models to
start) live in `selection.json`. Settings that only affect the UI live in local storage.

### The index

```
meta(key, value)                      embedding model + dimension, parser version
folders(id, path, added_at, status)
files(id, folder_id, path, hash, mtime, size, kind, indexed_at)
chunks(id, file_id, ordinal, text, page)
vec_chunks(chunk_id, embedding)       sqlite-vec virtual table, cosine distance
```

- A file is re-read only when it changed: modified time and size are compared first, then a
  blake3 hash of the content.
- The vectors belong to one embedding model. Selecting another one drops them and re-indexes
  every folder, because vectors from different models cannot be compared.
- `parser_version` lets a later release re-read files that yielded no text under older
  parsers (this is how scanned PDFs were picked up when OCR arrived).

## Main flows

### Start-up

1. Load `selection.json`, open `loft.db`, dock the window on the chosen display and edge.
2. Start the embedding server, then the chat server, for the selected models.
3. When the embedding server is ready, start a watcher on every folder and queue each for an
   incremental index run.
4. A supervisor checks the two servers every 3 seconds and restarts one that has exited (at
   most 3 times in 5 minutes).

### Indexing a folder

```
walk ─► parse ─► chunk ─► embed ─► store
```

- **Walk**: honours `.gitignore`, skips hidden folders, dependency and build folders, lock
  files and files over the size limit (5 MB text, 25 MB pictures, 100 MB documents).
- **Parse**: by extension. PDF pages with no text layer and pictures go through OCR.
- **Chunk**: up to 1,600 characters with 200 of overlap, split at paragraph boundaries; each
  chunk keeps its page, slide or sheet number.
- **Embed**: 16 chunks per request; the file name is prepended to give the vector context.
- **Store**: a file's chunks and vectors are replaced in one transaction.

One index run happens at a time across all folders. Requests made while a run is in flight
are merged into one follow-up run. File changes are debounced (1.5 seconds of quiet, 10
seconds at most) before they queue a run.

With a picture model selected, each picture is also described: the model is started the first
time a run needs it and stopped when the run ends.

### Answering a question

```
question ─► [rewrite if follow-up] ─► embed ─► search ─► select ─► prompt ─► stream
```

1. **Rewrite** (follow-ups only): the chat model turns "Where?" into a question that stands
   on its own, guided by four worked examples. If the result is unusable, the previous
   question is searched for together with the new one.
2. **Search**: the 6 nearest chunks by cosine similarity.
3. **Select**: nothing if the best match is below 0.45; otherwise the matches within 0.12 of
   the best, up to about 6,400 bytes.
4. **Prompt**: rules, up to 4 earlier turns (about 2,400 bytes), the excerpts labelled with
   file and location, and the question.
5. **Stream**: tokens are forwarded to the UI as events. Token counts come from the server's
   own usage figures; the context size comes from its `/props`.

A newer question or the Stop button bumps a generation counter; a stream whose number is no
longer current stops within 200 ms.

### Chat options

The options sheet changes single rules of one tuned system prompt, so the default options
leave that prompt byte-for-byte as tested:

| Option | Effect on the request |
|---|---|
| Answer length | Swaps the last rule; Concise is also capped at 150 tokens. |
| Tone | Appends one rule. |
| Answer from my files only, off | Swaps the "use only the excerpts" and refusal rules for "answer from general knowledge, starting with *Your indexed files don't cover this*". |
| Temperature, max tokens | Passed to the server. |

The remaining options (citations, source panel, streaming, memory, history) are handled in
the UI.

## The UI contract

**Commands** (UI → core): `get_runtime_status`, `list_models`, `select_model`,
`set_picture_model`, `download_model`, `list_folders`, `add_folder`, `reindex_folder`,
`delete_index`, `remove_folder`, `ask`, `cancel_ask`, `open_source`, `set_expanded`,
`get_placement`, `set_placement`, `list_monitors`, `set_monitor`, `get_autostart`,
`set_autostart`, `quit_app`, and `dev_log` (debug builds).

**Events** (core → UI):

| Event | Carries |
|---|---|
| `runtime:status` | State of the chat and embedding servers, and CPU or GPU. |
| `folders:changed` | Something about a folder changed; the UI re-reads the list. |
| `download:progress`, `download:error` | Model download progress. |
| `ask:sources` | The excerpts chosen for an answer, before any text is written. |
| `ask:token` | One piece of the streamed answer. |
| `loft://toggle` | The hotkey or tray icon asked to show or hide the panel. |

## The window

Loft is one transparent, undecorated, always-on-top window. A transparent window still
swallows clicks, so it is kept only as large as what is drawn plus a margin for the shadow:

| Placement | Pill | Panel |
|---|---|---|
| Top edge | 320 × 76 | 540 × 580 |
| Right edge | 76 × 274 | 520 × 600 |

The UI asks the core to grow the window just before the panel opens and to shrink it after
the closing animation. The window is always placed on the display chosen in Settings, never
on "whichever display it is mostly on": while it resizes at a screen edge that can be the
neighbouring display. For the same reason it is moved before it grows and shrunk before it
moves.

## Running the models

- **Which build**: a GPU build (CUDA, then Vulkan; Metal on Apple silicon) when its driver
  library is present, otherwise the CPU build.
- **Which GPU**: on a machine with several, the discrete GPU with the most memory is passed
  to llama.cpp; otherwise it would split a model across integrated and discrete GPUs.
- **How many layers**: left to llama.cpp, which fits what it can in free VRAM.
- **Failure**: a server that cannot start on the GPU build is retried once on the CPU build.
- **Clean-up**: servers are stopped on exit. On Windows they are also placed in a job object,
  so they die with Loft even if it is killed.

## Decisions worth knowing

- **The model decides when to decline.** On a small folder, similarity scores for unrelated
  and relevant questions overlap, so the similarity floor only removes the clearly unrelated
  and the model is told to say when the excerpts do not hold the answer.
- **No inline citation numbers.** Small models get them wrong; sources are listed under the
  answer instead.
- **Excerpts are data.** The prompt tells the model that file contents are not instructions.
- **OCR comes from the operating system.** Windows' engine and PDF renderer need no download.
  Other systems return no text for now.
- **The picture model is small and on demand.** It gets the gist of a picture and invents
  detail when it runs on, so its reply is cut to two sentences and labelled as automatic.
- **Cited files are opened carefully.** Only files in the index, only document and picture
  types; everything else is revealed in the file manager, so a click can never run a script.
- **Downloads are verified.** Every model file must match the SHA-256 pinned in the catalog.

## Testing

| Layer | How |
|---|---|
| Unit tests | `cargo test --lib`: parsing, chunking, store, prompts, options, docking maths. |
| Real models | Ignored tests such as `rag::tests::end_to_end` index `sample-docs/` and check answers, declines, follow-ups, scans and options against the downloaded models. |
| Real UI | `scripts\win-smoke.bat` runs a script inside the app's webview that drives every feature and checks the window's size and position. |

The UI test uses a throwaway profile: in debug builds `LOFT_DATA_DIR` moves the index and
settings elsewhere, `LOFT_SEED_FOLDER` registers a folder at launch, and `LOFT_DEV_EVAL`
names the script to run. The user's own folders and settings are not read or changed.

## Known limits

- Built and tested on Windows only. The macOS and Linux paths exist but have never been run.
- Code is indexed as plain text: chunks ignore function boundaries and search is by meaning
  only, with no keyword search.
- An answer sees at most 6 excerpts, so "summarise everything" covers only what matched.
- The conversation is held by the UI and sent with each question; the core keeps no chat
  state.
