# Loft

A local **RAG notch app** that answers questions about the files in folders you choose —
fully offline, powered by **llama.cpp**. Add/remove folders, (re)index or delete their indexed
data, pick small models from a built-in list, and ask questions with cited sources. Runs on
**macOS, Linux and Windows**. Looks like a macOS notch: a pill at the top-center that expands.

> Status: **all planned milestones (M0–M6) are implemented.** Built and tested on Windows 11
> (CPU and Vulkan GPU). The macOS and Linux paths are written but have **not been built or run**.
> See [Known gaps](#known-gaps).
>
> On Windows, build and run through `scripts\win-dev.bat` (it selects the complete MSVC toolset).
> `sample-docs/` is a small folder for trying it out (its `scans/` hold a picture and a scanned
> PDF, to try OCR).

## How answers work

1. The question is embedded and the 6 nearest chunks are fetched from the index.
2. Chunks well below the best match are dropped; the rest (up to ~6 KB) go into the prompt.
3. The chat model answers from those excerpts only, streaming token by token. It is told to
   reply "I couldn't find that in your indexed files." when the excerpts don't hold the answer;
   in that case no sources are shown.
4. The answer leads; its sources are in an "N sources" drawer under it (open for the newest
   answer by default; see the Source panel option). The drawer shows the excerpts, and clicking one opens documents and pictures (PDF, Office, txt,
   md, png, jpg…) in their default app; code and scripts are only revealed in the file
   manager, never run. A PDF opens at the cited page when the default PDF app is a browser
   (Edge, Chrome, Firefox…), Acrobat or SumatraPDF on Windows; otherwise it just opens.

### Scans and pictures (OCR)

Pictures (png, jpg, bmp, gif, tiff, webp) are indexed by the text that can be read off them,
and PDF pages with no text layer (scans) are rendered and read the same way, page by page.
A picture with no readable text is not indexed.

- **Windows only for now.** It uses the OCR engine and PDF renderer built into Windows 10/11,
  so nothing is bundled or downloaded; the language is the Windows display language. On macOS
  and Linux pictures and scanned pages yield no text.
- At most 150 pages of one PDF are read by OCR (about 0.3 s a page).
- OCR reads text, not content: a photo, chart or diagram without words in it is only found
  when picture descriptions are on (below).
- Files that had no readable text before this was added are read again on the next launch.

Similarity alone cannot tell an unrelated question from a real one on a small folder (the
scores overlap), so the similarity floor only filters the clearly unrelated and the model does
the rest. Small models are not asked for inline citation numbers because they get them wrong.

### Chat & Answers options

The sliders button beside Send opens a sheet of options. They are remembered between launches
(in the webview's local storage) and never leave the computer; the button turns accent-coloured
when anything differs from the defaults.

| Option | What it does |
|---|---|
| **Answer length** | Concise (one short sentence, capped at 150 tokens), Balanced (one to three sentences) or Detailed (everything relevant). Swaps one rule of the prompt. |
| **Response tone** | Natural, Professional (structured and direct) or Technical (exact names and values, code in backticks). Adds one rule. |
| **Show citations** | Off hides the sources under answers. |
| **Source panel** | Auto: the newest answer's sources are open, earlier ones folded. Always expanded. Collapsed by default. A drawer you open or close by hand stays that way. |
| **Answer from my files only** | On (default): a question the files don't cover gets "I couldn't find that in your indexed files." Off: the model may answer from general knowledge, starting with "Your indexed files don't cover this." and listing no sources. |
| **Stream responses** | Off holds the answer back until it is complete. |
| **Remember conversation context** | Off sends each question on its own, so follow-ups are not understood. |
| **Chat history** | Don't save (forgotten when the panel closes), Current session only (until Loft quits), or Save locally (kept in local storage, up to 40 turns, and restored at the next launch). |
| **Temperature** | 0 to 1, default 0.2. Higher is more varied and less reliable with a small model. |
| **Maximum output tokens** | 256, 512, 1,024 (default) or 2,048: a hard ceiling on an answer. |

Reset puts everything back; Copy settings copies them as text. With the default options the
prompt is exactly the one the answer tests were tuned on.

### Picture descriptions (optional)

Settings → **Picture descriptions** turns on a small vision model (SmolVLM 500M, a 607 MB
download in two files). While a folder is indexed, each picture gets a one- or two-sentence
description, stored with any text OCR read off it, so "which picture shows a bar chart?"
finds a chart that has no words on it. Off by default.

- The model runs only while pictures are being described (about a second each on a GPU) and
  is stopped when the indexing run ends, so it uses no memory otherwise.
- Descriptions are approximate. A model this small gets the gist ("a lake with snow-capped
  mountains", "a bar chart with four coloured bars") and invents details if allowed to go on,
  so its reply is cut back to its first two sentences and stored as "Automatic description
  (may be imprecise)". Do not rely on it for what a chart's numbers are.
- Turning it on, off, or changing the model reads every indexed picture again.
- Pictures inside PDFs and Office files are not described.

### Context usage

Under each answer Loft shows how many tokens the answer took and how fast it was written
("58 tokens · 61/s"). Below the question box a meter shows how full the model's context window
was for the latest answer ("Context 2% · 640 of 32,768 tokens · 2 turns remembered"); hovering
it breaks the prompt down into excerpts, earlier turns and the question, and says when older
turns were left out. The numbers are the chat server's own counts, and the window size is what
the server reports for a request, not the model's theoretical maximum. A question that is
refused without calling the model, or stopped, has no numbers.

### Follow-up questions

The Ask tab is a conversation: a question is answered with the turns before it, so "Where?"
after "When does the pilot start?" works. **New chat** starts over.

- The conversation lives in the panel only. It is sent with each question; nothing is stored,
  and it is gone when Loft quits.
- Before searching, the chat model rewrites a follow-up into a question that stands on its own
  ("Where does the pilot start?"), because searching for "Where?" finds nothing useful. If the
  rewrite is unusable, the previous question is searched for together with the new one. This
  costs one short extra model call per follow-up; a first question skips it.
- The prompt then holds the last 4 turns at most (about 2,400 bytes; long answers are cut to
  700 characters, oldest turns dropped first), the fresh excerpts, and the question.
- Limits: the model can answer a follow-up from what was already said rather than from the new
  excerpts, in which case the sources listed under it may not be where the answer came from.
  Turns older than the last 4 are forgotten.

## Architecture (summary)

```
Notch UI (Svelte + Vite)  ──IPC──►  Rust core (Tauri 2)
                                       ├─ runtime: spawn/supervise llama-server (chat + embed)
                                       ├─ models:  catalog, download, select
                                       ├─ index:   walk/parse/chunk/embed, FS watch
                                       ├─ store:   sqlite-vec (vectors + metadata)
                                       └─ rag:     retrieve → prompt → stream
                                     │                         │
                              llama-server (chat)      llama-server (embed)
```

Two local `llama-server` processes: one for chat, one dedicated to embeddings (embedding models
are pooling-based and don't reliably share a process with chat).

## Prerequisites

| Tool | Notes |
|---|---|
| Node.js 18+ | frontend + Tauri CLI |
| Rust (rustup, stable) | Tauri backend |
| Windows: MSVC C++ build tools + Windows 10/11 SDK | Rust needs these to link |
| Windows: WebView2 runtime | the webview Tauri renders into (preinstalled on Windows 11) |
| llama.cpp server builds | fetched by `npm run fetch-llama` into `src-tauri/binaries/` (not committed) |
| Linux: `libayatana-appindicator3` | for the tray icon |

### Installing the Windows build toolchain (one time)

```powershell
# 1) Rust (MSVC toolchain)
winget install --id Rustlang.Rustup -e

# 2) MSVC C++ build tools + Windows SDK (adds "Desktop development with C++")
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Then open a **new** terminal so the updated PATH is picked up, and run `rustc --version` to confirm.

## Run

### Notch UI only (no Rust needed)

```bash
npm install
npm run dev      # http://localhost:1420  (mock data, full UI)
```

### Full desktop app

```bash
npm install
npm run fetch-llama        # once: downloads the llama.cpp server builds for this OS
npm run tauri icon ./app-icon.svg   # once: generates src-tauri/icons (not checked in)
npm run tauri dev          # macOS / Linux
scripts\win-dev.bat        # Windows (selects the complete MSVC toolset first)
```

Then: **Settings** (the gear) → download and select a chat model and an embedding model →
**Folders** tab → add a folder → **Ask**. Ctrl/Cmd+Shift+Space summons the notch.

Settings also holds:

- **Theme**: Midnight, Graphite, Ember or Light (remembered in the webview's local storage).
- **Placement**: where the notch is docked. *Top* is the centre of the top edge; *Right* is the
  middle of the right edge, where the pill stands upright and the panel opens leftwards.
  Remembered in `selection.json`, next to the model selection.
- **Display**: which monitor the notch is on (listed under Placement; the main display by
  default). If the chosen display is unplugged, Loft falls back to the main one.
- **Launch at login** and **Quit** (Quit is on the tray icon too).

### Installers

```bash
npm run tauri build        # macOS / Linux
scripts\win-build.bat      # Windows -> src-tauri/target/release/bundle/{nsis,msi}/
```

The llama.cpp builds in `src-tauri/binaries/` are bundled into the installer; models are not
(they are downloaded in the app, and verified against the SHA-256 pinned in `catalog.json`).
Installers are **unsigned**: Windows SmartScreen and macOS Gatekeeper will warn until you sign
them with your own certificates.

## GPU acceleration

`npm run fetch-llama` fetches a CPU build and, where one exists, a GPU build (Vulkan on Windows
and Linux, Metal on Apple silicon). At startup Loft picks the GPU build when the machine has
the driver for it, otherwise the CPU build. The badge in the panel header shows which.

- On a machine with an integrated and a discrete GPU, Loft passes the discrete one to
  llama.cpp (`--device`); otherwise llama.cpp would split the model across both.
- llama.cpp decides how many layers fit in free VRAM, so a small GPU still helps.
- If a server cannot start on the GPU build, Loft retries once on the CPU build and says so in
  the badge's tooltip.
- CUDA: not fetched by the script. Unpack the two CUDA archives of the same llama.cpp release
  into `src-tauri/binaries/<platform>-cuda/` and it takes priority over Vulkan. Untested.

## Reliability

- A model server that dies is restarted with the same model (up to 3 times in 5 minutes).
- On Windows the servers are tied to Loft's process, so they cannot outlive it even if Loft is
  killed. On macOS/Linux they are stopped on a normal quit only.
- The window is only as large as what is drawn, including the margin its shadow needs (top:
  pill 320×76, panel 540×580; right: pill 76×274, panel 520×600), so it does not block clicks
  on what is underneath.

## Testing

```bash
cd src-tauri
cargo test --lib                                              # unit tests
cargo test --lib rag::tests::end_to_end -- --ignored --nocapture      # real models: answers
cargo test --lib indexer::tests::end_to_end -- --ignored --nocapture  # real models: indexing
cargo test --lib models::tests::installed_models_match_catalog -- --ignored  # checksums
```

To exercise the real UI in the native window without clicking, a debug build runs the script
named by `LOFT_DEV_EVAL` inside the webview once the servers are up. `scripts/smoke-ui.js`
drives ask / sources drawer / follow-ups / context stats / New chat / scans / every chat option / stop / switching
tabs mid-answer / rebuild / delete index / settings / themes / placement / displays / picture
descriptions / quit and prints
each step (with the window's size) to the terminal:

```powershell
scripts\win-smoke.bat
```

The test runs in a throwaway profile (`%TEMP%\loft-smoke-profile`) whose only folder is
`sample-docs`: your own folders, index and settings are neither read nor changed. Two more
debug-build switches make that possible: `LOFT_DATA_DIR` puts the index and settings in
another folder (models are still read from the real one), and `LOFT_SEED_FOLDER` registers a
folder at launch. The picture-descriptions step needs the picture model to be downloaded.

## Known gaps

- **macOS and Linux are untested.** The code paths, `fetch-llama` entries and bundle config
  exist, but nothing has been built or run there. Window resizing of a non-resizable window
  and the tray icon are the likeliest things to need attention.
- **Not exercised by any test:** the native Add-folder dialog, the remove-folder button,
  clicking a source to open it, the tray icon and the global hotkey (they need a real click or
  key press), turning on Launch at login (only allowed in an installed build), and installing
  from the installers (they were built and their contents checked, not run).
- **Unsigned installers**, no auto-updater.
- Models, the index (`loft.db`) and the model selection live in the app-data folder named
  after the bundle identifier `com.loft.desktop` (on Windows: `%APPDATA%\com.loft.desktop`).
  Uninstalling does not remove it.
- Picture descriptions were judged on about a dozen pictures (stock wallpapers, drawn charts,
  a document), not on personal photos, and only with SmolVLM 500M. A larger vision model
  would describe more accurately; none is in the catalog yet.
- OCR (scanned PDFs, pictures) works on Windows only, and has been tried only on clean,
  computer-drawn sample scans, not on photographed or skewed pages. Opening a PDF at its page
  is built for the common Windows PDF apps but has not been clicked through by hand.
- Answer and follow-up tuning was done on a four-file sample folder with Qwen 2.5 3B + Nomic
  embeddings. Other chat models may rewrite follow-ups less reliably.
- Twice, a single question has taken 13–19 s to start (once on the first answer after launch,
  once before the sources appeared) on the Vulkan build, and was fast when repeated. The cause
  is not known.

## Project layout

```
src/                      # notch UI (Svelte 5 + Vite)
  App.svelte              # notch shell: pill ⇄ panel, tabs
  lib/components/         # Ask, Folders, Settings (themes, Models, general)
  lib/theme.svelte.ts     # theme list + remembered choice (colours are in app.css)
  lib/api.ts              # Tauri commands (+ browser mocks)
  lib/types.ts            # DTOs shared with Rust
src-tauri/                # Rust backend (Tauri 2)
  src/lib.rs              # commands, window sizing, tray, hotkey, crash restart
  src/runtime.rs          # llama-server processes, GPU/CPU build selection
  src/models.rs           # model downloads
  src/store.rs            # sqlite-vec store
  src/parse.rs, chunk.rs, indexer.rs, jobs.rs   # indexing + file watching
  src/ocr.rs              # text recognition for pictures and scanned PDF pages (Windows)
  src/vision.rs           # optional picture descriptions from a small vision model
  src/rag.rs, opener.rs   # answers + opening sources
  catalog.json            # curated model catalog (GGUF URLs)
  tauri.conf.json         # transparent, top-center, always-on-top window; bundle config
scripts/
  fetch-llama.mjs         # download + verify the llama.cpp builds for this OS
  smoke-ui.js             # drives the real UI in a debug build (see Testing)
  win-dev.bat, win-build.bat, msvc-env.bat   # Windows build environment
app-icon.svg              # icon source → `npm run tauri icon`
```

## Milestones

- **M1** llama.cpp runtime: spawn + supervise the two servers.
- **M2** Model manager: download with progress + checksum, select chat + embed.
- **M3** Store & embeddings: sqlite-vec schema, embed + KNN query.
- **M4** Indexing: parsers (text/code/md/PDF/Office), chunker, incremental hashing, FS watch.
- **M5** RAG answers: retrieve → grounded prompt → streamed answer + sources; follow-up
  questions within a conversation.
- **M6** Polish & packaging: tray + quit, window sizing, GPU builds with CPU fallback, crash
  restart, launch at login, checksums, Windows installers.
