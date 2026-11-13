# Loft architecture

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
│    engine    which engine and device run each model; safe switching          │
│    runtime   find the engines; start and stop the model servers              │
│    models    catalog, downloads, checksums                                   │
│    indexer   walk → parse (+OCR) → chunk → embed → store                     │
│    jobs      index queue, file watching, folder commands                     │
│    store     SQLite + sqlite-vec: files, chunks, vectors                     │
│    rag       retrieve → prompt → streamed answer                             │
│    history   saved conversations, messages and their sources                 │
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

Nothing leaves the computer except downloads that the user starts: model files from Hugging
Face, the llama.cpp builds fetched by `npm run fetch-llama` at setup, and an optional engine
(CUDA, OpenVINO) installed from Settings. The model servers listen on `127.0.0.1` only.

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
| `lib/components/History.svelte` | The conversations panel: search, date groups, resume, rename, delete. |
| `lib/components/Folders.svelte` | Folder list with status, progress and actions. |
| `lib/components/Settings.svelte` | Theme, placement, display, models, performance, launch at login, quit. |
| `lib/components/Models.svelte` | Chat, embedding and picture models: download, select. |
| `lib/components/Performance.svelte` | Performance & battery: what is running, the two choices, trying an engine or the NPU, installing engines, logs. |
| `lib/components/Benchmark.svelte` | Test performance: progress, results, recommendations. |
| `lib/api.ts` | Every call to the backend, with mock data when run in a plain browser. |
| `lib/state.svelte.ts` | Panel state: expanded, current tab, answer in progress. |
| `lib/chat-options.svelte.ts` | Chat options and their persistence. |
| `lib/theme.svelte.ts`, `lib/placement.svelte.ts`, `lib/window-style.svelte.ts` | Theme, notch placement, and window style (notch or floating, living in the tray). |
| `lib/types.ts` | Data shapes shared with the Rust side. |
| `app.css` | Theme variables and shared styles. |

The three tab views are always mounted and shown or hidden, so an answer keeps streaming
while another tab is open. `api.ts` detects whether it is inside Tauri; outside it serves
mock data, which is how the UI is developed and previewed without building the backend.

### Backend: `src-tauri/src/`

| Module | Role |
|---|---|
| `lib.rs` | App state, all Tauri commands, window sizing and docking, tray, hotkey. |
| `inference.rs` | Engines and devices as data, and every rule about them: what a choice resolves to, what "automatic" tries, whether to keep, wait, restart or stop, how to launch a target, how to read a server's load log, power debouncing, NPU compatibility. No processes and no Tauri types, so all of it is unit-tested. |
| `engine.rs` | The transition coordinator: the one place servers are started, switched and restarted (settings, model choice, temporary engine, power change, crash recovery). |
| `runtime.rs` | Finds the engine builds, asks each which devices it sees, starts and stops `llama-server`, and keeps what each server logs. |
| `hardware.rs`, `power.rs` | What the operating system reports: graphics cards, NPUs and their drivers; mains or battery, change notifications, battery drain. |
| `installer.rs` | Optional engines: the pinned list, download with size and SHA-256 checks, unpacking by file name only, swap-in. |
| `bench.rs` | Test performance, "Try … now", stored measurements and the advice drawn from them. |
| `usage.rs` | "What Loft is using": each part's processes, resident memory and processor share, plus model, context and graphics memory from the server's load log. |
| `models.rs` | Downloads a model file with resume, checks the GGUF header and the SHA-256. |
| `parse.rs` | Turns a file into text sections: text and code, PDF, docx, pptx, spreadsheets, pictures. |
| `ocr.rs` | Text recognition for pictures and scanned PDF pages (Windows' built-in engine). |
| `chunk.rs` | Splits sections into overlapping chunks on paragraph boundaries. |
| `embed.rs` | Client for the embedding server. |
| `indexer.rs` | Brings one folder's index up to date. Free of Tauri types, so it is tested directly. |
| `jobs.rs` | Index queue, file watchers with debouncing, folder commands, progress events. |
| `store.rs` | The SQLite database: folders, files, chunks, vectors, search. |
| `rag.rs` | Retrieval, prompts, chat options, follow-up rewriting, answer streaming, usage stats. |
| `history.rs` | Chat history: conversations, messages and each answer's sources, in their own database. Storage only: it builds no prompt. |
| `vision.rs` | Starts the picture model on demand and writes short picture descriptions. |
| `opener.rs` | Opens a cited file safely, a PDF at its page where possible. |

## Data

| What | Where |
|---|---|
| Models (`*.gguf`) | `<app data>/models/` |
| Index (`loft.db`) | `<app data>/` |
| Model selection, window style and position, placement, display, picture model, engine choices (`selection.json`) | `<app data>/` |
| Performance measurements (`benchmarks.json`) | `<app data>/` |
| Optional engines installed from Settings | `<app data>/engines/` |
| Chat history (`chats.db`) | `<app data>/` |
| Theme, chat options, which conversation was open | the webview's local storage |
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
2. Find the engine builds and ask each one which devices it can use (each is started once,
   briefly, with `--list-devices`).
3. Reconcile: start the embedding server, then the chat server, on whatever the saved choice
   for the current power source resolves to.
4. When the embedding server is ready, start a watcher on every folder and queue each for an
   incremental index run.
5. From then on: watch the power source, and check the two servers every 3 seconds. A server
   that has exited is restarted through the same reconcile step (at most 3 times in 5
   minutes).

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
3. **Select**: nothing if the best match is below the embedding model's floor (`minScore` in
   the catalog: 0.45 for Nomic, 0.20 for EmbeddingGemma, 0.45 where none is given); otherwise
   the matches within 0.12 of the best, up to about 6,400 bytes.
4. **Prompt**: rules, up to 4 earlier turns (about 2,400 bytes), the excerpts labelled with
   file and location, and the question.
5. **Stream**: tokens are forwarded to the UI as events. Token counts come from the server's
   own usage figures; the context size comes from its `/props`.

A newer question or the Stop button bumps a generation counter; a stream whose number is no
longer current stops within 200 ms.

### Chat history

```
conversations(id, title, title_manual, created_at, updated_at, opened_at, pinned, archived)
messages(id, conversation_id, ordinal, role, content, created_at, status, model, stats, error)
message_sources(message_id, ordinal, file, path, page, location, score, snippet, file_hash)
```

Ids are GUIDs; times are UTC milliseconds. `chats.db` is a separate file from the index, so
neither can damage the other: dropping every vector (a new embedding model) leaves the
conversations alone, and deleting a conversation cannot reach a file.

- **History is storage; context is inference state.** The UI shows a saved conversation in
  full and sends its answered turns with the next question. `rag::recent_history` then keeps
  the last 4 within 2,400 bytes, exactly as for an unsaved conversation. `history.rs` never
  builds a prompt, and nothing is summarised.
- **A turn is written in two steps.** `ask` saves the question and opens the answer's row as
  `pending` before anything is generated, and announces the conversation with `ask:saved`.
  When the stream ends it fills the row in as `complete`, `stopped` (the generation counter
  moved: Stop, or a newer question) or `failed` (with the error and whatever text had
  arrived). Rows still `pending` at start-up were cut off by Loft closing and become
  `interrupted`. Text that is not a finished answer is therefore never stored as one.
- **Events carry their question's id.** The UI passes a request id to `ask`; `ask:saved`,
  `ask:sources` and `ask:token` carry it back, and the UI listens for that id only. A
  stopped answer's last tokens cannot be appended to the next one. The UI also refuses to
  open another conversation while an answer is being written, and counts each replacement
  of the conversation on screen so an answer that ends afterwards cannot attach itself.
- **Citations are a record, not a link.** Each is stored as shown, with the indexed file's
  content hash at that moment. On reopening, the hash of whatever is indexed at that same
  path now decides: same, changed, or missing. `open_source` already refuses anything that
  is not in the index, so a stale citation cannot open an unrelated file.
- **A failed write does not cost the answer.** If the turn cannot be saved the answer is
  still returned, with `saveError` set, and the UI says it was not saved.
- Commands that read or write history are `async`, so they run off the UI thread.

### Chat options

The options sheet changes single rules of one tuned system prompt, so the default options
leave that prompt byte-for-byte as tested:

| Option | Effect on the request |
|---|---|
| Answer length | Swaps the last rule; Concise is also capped at 150 tokens. |
| Tone | Appends one rule. |
| Answer from my files only, off | Swaps the "use only the excerpts" and refusal rules for "answer from general knowledge, starting with *Your indexed files don't cover this*". |
| Temperature, max tokens | Passed to the server. |

Two things are worked out per question rather than chosen (`rag.rs`):

- **The decline names the folders.** `decline_message` is "I couldn't find that in your
  indexed files." followed by "I'm only really here to talk about items in A, B and C." (the
  last part of each folder's path; more than four become "A, B, C and N other folders"). Loft
  says it itself when nothing in the index is close to the question, and the refusal rule of
  the system prompt tells the model to say exactly that when the excerpts do not hold the
  answer. With no folders the rule, and so the prompt, is the tuned one unchanged.
- **Small talk skips the index.** `is_small_talk` recognises greetings, thanks, goodbyes and
  "who are you" from a fixed list: at most 8 words, every one part of a listed phrase or a
  filler word, so a real question is never taken for one. Such a message goes to the chat
  model with its own short prompt (be brief and warm, say what Loft is for, state nothing
  about the files), 80 tokens at most, no excerpts and no earlier turns. Small-talk turns are
  also left out of the conversation memory sent with a follow-up.

The remaining options (citations, source panel, streaming, memory) are handled in the UI.
"Chat history" decides whether a new conversation is saved (`ask` is called with `save`); a
conversation that was opened from the history goes on being saved either way.

## The UI contract

**Commands** (UI → core): `get_runtime_status`, `list_models`, `select_model`,
`set_picture_model`, `download_model`, `list_folders`, `add_folder`, `reindex_folder`,
`delete_index`, `remove_folder`, `ask`, `cancel_ask`, `list_conversations`,
`get_conversation`, `rename_conversation`, `delete_conversation`, `import_conversation`,
`open_source`, `set_expanded`,
`get_placement`, `set_placement`, `list_monitors`, `set_monitor`, `get_window_settings`,
`set_window_style`, `set_tray_only`, `remember_position`, `get_autostart`,
`set_autostart`, `quit_app`; for engines `get_inference`, `set_inference_settings`,
`set_engine_override`, `try_engine`, `retry_engines`, `get_engine_log`, `install_engine`,
`cancel_engine_install`, `remove_engine`, `run_benchmark`, `get_benchmarks`,
`cancel_benchmark`; and in debug builds `dev_log`, `dev_set_power`, `dev_fail_target`,
`dev_kill_engine`, `dev_engines`, `dev_summon`.

Commands that touch engines take a backend name or a target (backend plus device key) and
nothing else: no path, URL or command line ever comes from the UI.

**Events** (core → UI):

| Event | Carries |
|---|---|
| `runtime:status` | State of the chat and embedding servers, and the engine in use. |
| `inference:status` | For each server: what was asked for, what is really running and whether the log confirmed it, fallback and its reason, a pending switch; the power source. |
| `engine:install` | Progress of an optional engine's download and install. |
| `bench:progress` | Which engine the performance test is on. |
| `folders:changed` | Something about a folder changed; the UI re-reads the list. |
| `download:progress`, `download:error` | Model download progress. |
| `ask:saved` | The question has been stored: its conversation, and the id of the answer being written. |
| `ask:sources` | The excerpts chosen for an answer, before any text is written. |
| `ask:token` | One piece of the streamed answer. (All three carry the request id the UI gave the question.) |
| `loft://toggle` | The hotkey or tray icon asked to show or hide the panel. |

## The window

Loft is one transparent, undecorated, always-on-top window. A transparent window still
swallows clicks, so it is kept only as large as what is drawn plus a margin for the shadow:

| Placement | Pill | Panel |
|---|---|---|
| Top edge, or floating | 320 × 76 | 540 × 580 |
| Right edge | 76 × 274 | 520 × 600 |

The UI asks the core to grow the window just before the panel opens and to shrink it after
the closing animation. The window is always placed on the display chosen in Settings, never
on "whichever display it is mostly on": while it resizes at a screen edge that can be the
neighbouring display. For the same reason it is moved before it grows and shrunk before it
moves.

**Two styles.** The *notch* is docked: its position is computed from the chosen edge and
display every time. The *floating* window is placed from one saved point, the top-centre of
the window, so the pill and the panel that opens from it line up; the point is clamped so the
whole window is on the display that holds it, and a point on no display (a monitor was
unplugged) falls back to a default spot. The UI starts a native window drag from the pill (a
press that moves more than a few pixels; one that does not is a click) or from the empty part
of the panel's top bar. When a move has settled, the UI asks the core to remember where the
window is, unless Loft itself moved it in the last moment (opening, closing, changing style).

**Living in the tray.** The window starts hidden and is shown once it has been placed, or not
at all when Loft lives in the tray. In that mode closing the panel hides the window, and the
tray icon and the hotkey show it and open the panel. The setting is refused when the tray
icon could not be created, because nothing would be left to open Loft with.

## Running the models

A **backend** is a llama.cpp build (CUDA, Vulkan, OpenVINO, Metal, CPU); a **device** is the
processor that does the work. A **target** is the pair. The user saves one choice for mains
power and one for battery; each is *automatic* or a target.

```
settings + power source + temporary engine ──► plan      (inference::plan)
                                                 │         an ordered list of targets
plan + what is running + what failed + busy? ──► decision (inference::decide)
                                                 │         keep · wait · start · stop
                                                 ▼
                                     engine::reconcile carries it out, one at a time
                                                 │
                              start server ─► healthy? ─► read its load log (inference::verify)
                                                 │               │
                                             no: next target   wrong place: next target
```

- **One coordinator.** Model selection, the settings page, a temporary engine, a power
  change, crash recovery and the end of an install all call `engine::reconcile`. It holds
  one lock for the whole transition, so two triggers cannot restart the same server at once,
  and it compares before it acts, so a repeated request restarts nothing.
- **Work is never interrupted.** `ask` and indexing hold a `Busy` guard for the servers they
  use. A switch that finds a server busy is recorded as pending; dropping the last guard
  runs `reconcile` again.
- **Requested and active are different things.** The status reports both. "Active" is filled
  in only after the server's own log has been read: `using device <id> (<name>)` and
  `<id> model buffer size = N MiB` for GPUs, `OpenVINO: using device <id>` for OpenVINO.
  ("offloaded N/N layers to GPU" is printed even when nothing was, so it is ignored.) A
  server that is healthy but has the model in the wrong place is stopped and counts as a
  failure, and a failed transition never leaves the state at "ready".
- **Fallback is the user's call.** With fallback allowed, the plan continues down the
  automatic list and the status names the engine that failed and why. Without it the plan
  is the one target, and failure means stopped with the reason. Either way the saved choice
  is never rewritten, and a target that failed is skipped until the settings change or the
  user presses Retry, so nothing loops.
- **Devices have stable names.** llama.cpp numbers GPUs (`Vulkan0`, `Vulkan1`) in an order
  that can change. A saved choice holds a key made from the device's name and is matched to
  the current number at launch. A device that has gone stays in the list as unavailable.
- **CPU only means it**: `--device none`, checked against the log like any other target.
- **Power.** Windows delivers power-source changes through
  `PowerSettingRegisterNotification` (no polling, no window). Readings are debounced for
  three seconds, so duplicates and a loose charger do nothing.
- **Hardware list.** Graphics cards and NPUs come from SetupAPI device classes, with driver
  versions, not from DXGI: creating a DXGI factory in the same process as Windows' OCR and
  PDF engines stopped the process from exiting.
- **Measurements.** `bench.rs` runs each target alone with a fixed prompt while holding the
  coordinator's lock. Results carry the identity of what they were measured on and are
  ignored once it changes. Current results reorder "automatic"; energy counts only when it
  was measured on battery. One run varies from the next by about a tenth, so a fast engine
  is measured three times (middle kept) and engines within 15% of the best are not ranked
  against each other: the rule of thumb decides between them.
- **OpenVINO is found the hard way.** Its `--list-devices` prints nothing on the pinned
  Windows build, so its devices are read from the refusal it gives for a device that does not
  exist, and cached beside the engine (the refusal is logged by Windows as a crash).
- **Servers run in a scratch folder.** Engines leave files in their working directory
  (Intel's graphics compiler writes `kernel.errors.txt`), which must not be the install folder.
- **Names lead with the device**, which is what people recognise; the engine follows in
  brackets. The engine's own device name is kept unchanged underneath, for keys and for the
  identity of stored measurements.
- **Optional engines** are downloaded from the release pinned in `engines.json`, checked
  against the pinned size and SHA-256, unpacked by file name only into
  `<app data>/engines/`, and swapped in only when complete.
- **How many layers**: left to llama.cpp, which fits what it can in free VRAM.
- **No mapped copy on a graphics card.** For CUDA and Vulkan on a card with memory of its
  own, servers start with `--load-mode none`. With llama.cpp's default (the model file
  memory-mapped), the pages read while copying the weights to the card stay resident: for
  Qwen 2.5 3B on the RTX 4050 the chat server held 2.3 GB of RAM on CUDA and 2.0 GB on Vulkan;
  with `none`, 0.6 GB and 0.35 GB, with the same load time and answer. On Intel Iris Xe
  (memory shared with the system) it made no difference, so built-in graphics, Metal,
  OpenVINO and the CPU keep the default. The weights then sit in a `CUDA_Host` /
  `Vulkan_Host` buffer instead of `CPU_Mapped`; `verify` counts both as system memory.
- **Usage** (`usage.rs`, shown by `Usage.svelte`, a view opened from the header like Settings) lists Loft's process tree (Toolhelp on Windows,
  `/proc` on Linux, `ps` on macOS) and puts each process with its part: the two model
  servers by process id, any other `llama-server` as the picture model, the rest (the web
  view's processes) as Loft. Memory is resident memory (working set / RSS). Not committed or
  "private" memory: on Windows that also counts what a process has allocated on a graphics
  card, so the model would be counted twice. Graphics memory, model size and context size
  come from the load log. Memory on built-in graphics or an NPU is system memory and is
  already in the resident figure (measured on Iris Xe), so it is shown but not added again.
  Processor share is the change in processor time between two readings, over all cores.
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
- **Downloads are verified.** Every model file must match the SHA-256 pinned in the catalog,
  and every engine archive the size and SHA-256 pinned in `engines.json`.
- **Loft recommends, the user decides.** Automatic is a default, a measurement produces a
  recommendation with a button, and an experimental engine is offered with a warning rather
  than hidden. A choice the user made is never replaced by something Loft fell back to.
- **A cut-off answer is an error.** If the stream ends without its closing marker (the
  server went away), the answer is reported as failed instead of shown as complete.

## Testing

| Layer | How |
|---|---|
| Unit tests | `cargo test --lib`: parsing, chunking, store, prompts, options, docking maths. |
| Real models | Ignored tests such as `rag::tests::end_to_end` index `sample-docs/` and check answers, declines, follow-ups, scans and options against the downloaded models. |
| Real UI | `scripts\win-smoke.bat` runs a script inside the app's webview that drives every feature and checks the window's size and position. |
| Real engines | `scripts\win-smoke.bat smoke-engines.js` switches real servers between real devices through the Settings page, with simulated power changes, forced failures and a killed server, and counts processes after each step. |

The UI test uses a throwaway profile: in debug builds `LOFT_DATA_DIR` moves the index and
settings elsewhere, `LOFT_SEED_FOLDER` registers a folder at launch, and `LOFT_DEV_EVAL`
names the script to run. The user's own folders and settings are not read or changed.

## Known limits

- Built and tested on Windows only. The macOS and Linux paths exist but have never been run.
- An NPU has been tried on one machine with one model (see HANDOVER.md); CUDA on one
  laptop. Other NPUs, drivers and models are unknown.
- The relevance floor is per embedding model, because each model scores on its own scale
  (best match per question on the sample folder: Nomic 0.47 to 0.58 for unrelated questions
  and 0.57 to 0.84 for answerable ones; EmbeddingGemma 0.00 to 0.19 and 0.25 to 0.75). It has
  been measured for those two only: BGE-M3 takes the default.
- Energy use has not been measured: that needs the performance test to be run on battery.
- The engine choices are global, not per model.
- Code is indexed as plain text: chunks ignore function boundaries and search is by meaning
  only, with no keyword search.
- An answer sees at most 6 excerpts, so "summarise everything" covers only what matched.
- The conversation on screen is held by the UI and sent with each question; the core keeps
  no chat state between questions, only the saved history.
- Chat history search is a scan (`LIKE`) of titles and messages: fine for hundreds of
  conversations, not designed for many thousands.
