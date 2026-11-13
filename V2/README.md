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
3. The chat model answers from those excerpts only, streaming token by token. When the
   excerpts don't hold the answer, or the request isn't about the files at all, the reply is
   "I couldn't find that in your indexed files. I'm only really here to talk about items in
   Documents and Notes." (your own folder names; up to four are named, then "and 2 other
   folders"). No sources are shown with it.
4. Small talk is the exception: "hi", "thanks", "how are you?", "who are you?" get a short
   friendly reply from the chat model, with no search and no sources. Only short messages
   made up entirely of such phrases count; "hi, when does the pilot start?" is a question.
5. The answer leads; its sources are in an "N sources" drawer under it (open for the newest
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
| **Answer from my files only** | On (default): a question the files don't cover gets "I couldn't find that in your indexed files. I'm only really here to talk about items in …" with your folder names. Off: the model may answer from general knowledge, starting with "Your indexed files don't cover this." and listing no sources. |
| **Stream responses** | Off holds the answer back until it is complete. |
| **Remember conversation context** | Off sends each question on its own, so follow-ups are not understood. |
| **Chat history** | Save locally (the default: every conversation is kept in the [chat history](#chat-history)), Current session only (a new conversation lasts until Loft quits), or Don't save (forgotten when the panel closes). |
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

- The turns on screen are sent with each question as context. Saving them is a separate
  matter: see [Chat history](#chat-history).
- Before searching, the chat model rewrites a follow-up into a question that stands on its own
  ("Where does the pilot start?"), because searching for "Where?" finds nothing useful. If the
  rewrite is unusable, the previous question is searched for together with the new one. This
  costs one short extra model call per follow-up; a first question skips it.
- The prompt then holds the last 4 turns at most (about 2,400 bytes; long answers are cut to
  700 characters, oldest turns dropped first), the fresh excerpts, and the question.
- Limits: the model can answer a follow-up from what was already said rather than from the new
  excerpts, in which case the sources listed under it may not be where the answer came from.
  Turns older than the last 4 are forgotten.

### Chat history

Conversations are kept on this computer, in `chats.db` in the app-data folder, and nowhere
else. The bar above the conversation has the history button, the conversation's name (which
opens the history too) and **New chat**.

- **Saved as you go.** The question is stored the moment it is asked, and the conversation is
  named after it (the question, tidied and cut to about 48 characters; no model is asked, so
  the first answer is not delayed). The answer is stored when it ends, with the model that
  wrote it, its usage figures and the excerpts it was written from.
- **Only a finished answer counts as one.** An answer you stopped is kept as *stopped*, one
  that failed as *failed* with the reason, and one that was being written when Loft closed
  becomes *interrupted* at the next launch. Each is shown that way when the conversation is
  reopened.
- **The history panel** lays over the conversation; the window does not change size.
  Conversations are grouped by date, newest first, with the open one marked. Search covers
  titles and everything that was said. The ⋯ menu renames (a name you gave is never replaced
  by an automatic one) and deletes (after asking). Up and Down move through the list from the
  search box, Enter opens, Esc backs out.
- **Resuming** shows a conversation exactly as it was saved (nothing is asked of the model
  again), scrolled to its latest message, ready for a follow-up. Loft reopens the last
  conversation at launch.
- **History is not context.** A conversation may hold a hundred turns; the next question is
  still sent with the last 4 at most, within the same budget as before. The context meter
  shows what was actually sent.
- **Sources stay with their answers.** Each citation is saved as it was shown (file, path,
  page, excerpt, score, order) with a hash of the file's contents. Reopened later, a file
  that is no longer indexed is marked *no longer available* and cannot be opened; one whose
  contents have changed is marked *changed since*. A citation is only ever checked against
  its own path, never matched to another file.
- **Switching is safe.** While an answer is being written you cannot open another
  conversation (stop it first; what was written is kept, as stopped). Every piece of a
  streamed answer carries the id of its question, so text cannot land in another answer.
- **Deleting** a conversation removes its messages and their source records. It is a
  different database from the index: your files and their index are never touched, and
  deleting or rebuilding an index never touches a conversation.
- If a turn cannot be saved, the answer is still shown, with a note saying it was not saved.

Not built (the storage has room for them): pinning, archiving, export, clearing all history,
retention limits, encryption, per-conversation models, and summaries of long conversations.

## Architecture (summary)

```
Notch UI (Svelte + Vite)  ──IPC──►  Rust core (Tauri 2)
                                       ├─ runtime: find the engines, start/stop llama-server
                                       ├─ engine:  which engine runs what, safe switching, power
                                       ├─ models:  catalog, download, select
                                       ├─ index:   walk/parse/chunk/embed, FS watch
                                       ├─ store:   sqlite-vec (vectors + metadata)
                                       └─ rag:     retrieve → prompt → stream
                                     │                         │
                              llama-server (chat)      llama-server (embed)
```

Two local `llama-server` processes: one for chat, one dedicated to embeddings (embedding models
are pooling-based and don't reliably share a process with chat).

[ARCHITECTURE.md](ARCHITECTURE.md) has the full picture: the code map, the data, the main flows
and the reasons behind the design.

## Prerequisites

| Tool | Notes |
|---|---|
| Node.js 18+ | frontend + Tauri CLI |
| Rust (rustup, stable) | Tauri backend |
| Windows: MSVC C++ build tools + Windows 10/11 SDK | Rust needs these to link |
| Windows: WebView2 runtime | the webview Tauri renders into (preinstalled on Windows 11) |
| llama.cpp server builds | fetched by `npm run fetch-llama` into `src-tauri/binaries/` (not committed); CUDA and OpenVINO are optional extras |
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
- **Window style**: *Notch* (docked at a screen edge) or *Floating* (a window of its own:
  drag it by its pill or by the top of its panel, and it stays where you leave it, on any
  display). Remembered in `selection.json`, next to the model selection.
- **Placement** (notch only): *Top* is the centre of the top edge; *Right* is the middle of the
  right edge, where the pill stands upright and the panel opens leftwards.
- **Display** (notch only): which monitor the notch is on (the main display by default). If
  the chosen display is unplugged, Loft falls back to the main one.
- **Live in the tray**: nothing stays on screen. Loft is opened and closed from its tray icon
  (the notification area on Windows, the menu bar on macOS, an app indicator on Linux) or
  with Ctrl/Cmd+Shift+Space, and works with either style. On macOS it also drops the Dock
  icon, from the next launch. Where the desktop gives Loft no tray icon the option is
  disabled, since Loft would be unreachable.
- **Performance & battery**: which engine and device run the models when plugged in and on
  battery, trying another engine (or the NPU) on the spot, and a performance test. See
  [Engines, performance and battery](#engines-performance-and-battery).
- **Launch at login** and **Quit** (Quit is on the tray icon too).

### Installers

```bash
npm run tauri build        # macOS / Linux
scripts\win-build.bat      # Windows -> src-tauri/target/release/bundle/{nsis,msi}/
```

The llama.cpp builds in `src-tauri/binaries/` are bundled into the installer; models are not
(they are downloaded in the app, and verified against the SHA-256 pinned in `catalog.json`).
Leave the large optional engines out of `binaries/` unless you want them in the installer:
users can add them from Settings.
Installers are **unsigned**: Windows SmartScreen and macOS Gatekeeper will warn until you sign
them with your own certificates.

## Engines, performance and battery

An **engine** is a llama.cpp build (the software path): CUDA, Vulkan, OpenVINO, Metal or CPU.
A **device** is the processor that does the work: an NVIDIA card, the Intel graphics built
into the processor, an Intel NPU, the CPU. Loft keeps the two apart, because one engine can
drive several devices (Vulkan: any GPU) and one device can be driven by several engines (an
NVIDIA card: CUDA or Vulkan).

**Settings → Performance & battery** is where you choose. Loft recommends; you decide.

| Control | What it does |
|---|---|
| What Loft is using | Live, every 2 seconds while Settings is open: memory, graphics-card memory and processor for the chat model, the embedding model, the picture model (while it reads pictures) and Loft's own window, with Loft's share of the computer's memory. Click a part for its model size, its context and the device it runs on. |
| When plugged in / When running on battery | The engine and device for each power state. *Automatic* lets Loft choose; anything else is your choice and is kept, even while it is unavailable. |
| Switch when the power source changes | On: the battery choice is used on battery. Off: the plugged-in choice always. |
| Allow fallback to another engine | On: if the chosen engine cannot run the model, the next best one is used and the status says so. Off: Loft stops and explains, and nothing else runs. |
| Try another engine now | Runs the chat model on any installed engine straight away, checks where it really is and asks it a test question. Temporary: it changes neither saved choice, and survives a change of power source until you press *Restore power profile*. |
| Try NPU now | The same, for an Intel NPU (through OpenVINO). Always shown; it says what is missing (the engine, the driver, the hardware) and offers to install the engine. |
| Devices this computer reports | Under Advanced: each graphics card and NPU with its driver version. |
| Test performance | Measures every installed engine with your chat model (see below). |
| Advanced engine controls | A separate choice for the embedding model, the list of engines with *Install* / *Remove* for the optional ones, the engines' logs, and *Look for engines again*. |

Choices are named after the device, with the engine after it:
`NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)`, `Intel Iris Xe Graphics (Vulkan)`, `CPU only`.

What the status card shows is what is **really running**, not what was asked for. After a
server starts, Loft reads the server's own load log: which device it said it was using and how
many megabytes of the model it put there. A server that is up but has the model somewhere
else (a GPU build that quietly kept it in system memory, "CPU only" that offloaded anyway)
counts as a failure. `Requested` and what is running are shown separately whenever they
differ, with the reason.

**Automatic** means:

- *Plugged in (performance)*: CUDA on a discrete card, then Vulkan on a discrete card, then
  Metal, then Vulkan on built-in graphics, then the CPU.
- *On battery (battery saving)*: built-in graphics, then the CPU, and a discrete card only as
  a last resort. This is a rule of thumb (the discrete card is the costliest part to wake),
  not a measurement.
- Once *Test performance* has results, the measured order replaces the rule of thumb: fastest
  first when plugged in. On battery it only does so when the test was run on battery **and**
  the battery reported its drain; speed alone is never treated as evidence about battery life.
- OpenVINO is experimental and is never chosen automatically unless it has been measured.

**Switching is safe.** Every start, switch and restart goes through one coordinator, one at a
time: choosing a model, changing these settings, a temporary engine, a change of power source
and crash recovery cannot collide. A repeated request does nothing. A power change must hold
for three seconds before it counts (a loose charger restarts nothing). A server in the middle
of an answer or an indexing run is left alone: the switch is shown as *pending* and happens
when the work ends. An engine that failed is not retried in a loop; *Retry* (or changing a
setting) tries it again. Changing the device never rebuilds the index: only a different
embedding **model** does.

### Engines

| Engine | Devices | How it gets here |
|---|---|---|
| CPU | the processor | `npm run fetch-llama` (bundled in the installer) |
| Vulkan | any GPU, each selectable by name | `npm run fetch-llama` (bundled) |
| Metal | Apple silicon | `npm run fetch-llama` on a Mac (bundled) |
| CUDA | NVIDIA cards | optional, about 590 MB: *Install* in Settings, or `npm run fetch-llama cuda` |
| OpenVINO (experimental) | Intel processors, graphics and NPUs | optional, about 105 MB: *Install* in Settings, or `npm run fetch-llama openvino` |

All of them are official prebuilt binaries of **one** llama.cpp release (`b12110`), pinned in
[`src-tauri/engines.json`](src-tauri/engines.json) together with the size and SHA-256 that
GitHub publishes for every archive. Loft and `fetch-llama` both refuse an archive that is not
under that release of `ggml-org/llama.cpp` or does not match its pinned size and checksum, and
unpack only the server, its libraries and their data files. The settings page only ever names
an engine; it cannot supply a URL or a path. To move to another release, change `TAG` in
`scripts/make-engines-manifest.py`, run it, and fetch again.

```bash
node scripts/fetch-llama.mjs --list      # what exists for this OS, and what is installed
node scripts/fetch-llama.mjs cuda        # an optional engine, into src-tauri/binaries/ (bundled)
```

An engine installed from Settings goes to `<app data>/engines/` instead and can be removed
there again.

### OpenVINO and the NPU: what "experimental" means here

llama.cpp's OpenVINO backend is new. What follows is what the pinned build (`b12110`,
OpenVINO 2026.4.1) actually did on the development laptop (Intel Core i7-12700H, Intel Iris Xe
and NVIDIA RTX 4050 graphics, **no NPU**), which is not always what its documentation says:

- The device is chosen with the environment variable `GGML_OPENVINO_DEVICE`, not `--device`.
  On a computer with two graphics devices the ids are `GPU.0` and `GPU.1`, not `GPU`. Loft
  sets it per server and then **checks**: the server must log `OpenVINO: using device <id>`
  for the device asked for, or the start counts as failed. Setting the variable is not taken
  as proof of anything.
- `--list-devices` prints nothing on this build. Loft learns OpenVINO's devices from the
  message the engine gives when asked for a device that does not exist (it names the ones
  that do). That refusal is an abort, which Windows records as a crash of `llama-server`, so
  the answer is kept in `loft-devices.json` beside the engine and asked for again only when
  the engine, the devices or their drivers change (or on *Look for engines again*).
- **It works on the Intel processor and on the Intel graphics**, slowly: about 4 and 6 tokens
  a second, against 11 for the plain CPU engine and 12 for Vulkan on the same graphics, and it
  takes 20 to 45 seconds to load the model.
- **It lists the NVIDIA card but cannot start on it** (the process dies while loading, without
  a message). Loft warns before such a choice and says afterwards to use CUDA or Vulkan.
- Embedding models are poorly supported, so with the chat model on OpenVINO the embeddings
  stay on another engine (the status says so).
- Intel's graphics compiler writes a `kernel.errors.txt` into the folder the engine runs in.
  Model servers are therefore started in a scratch folder under the temporary directory.
- The NPU, according to llama.cpp's documentation: it needs a small, fixed context (Loft uses
  4096 tokens there) and one request at a time, and runs only some models. Llama 3.2 3B,
  Gemma 3 4B and Phi-3.5 mini (Q4_K_M) are listed as passing; Loft's default Qwen 2.5 3B is
  not listed either way. It needs an Intel Core Ultra processor and the Intel NPU driver.

**On an NPU it has been run on one machine.** The development laptop has no NPU. On a second
Windows laptop (Intel Core Ultra 9 386H, Intel AI Boost NPU, driver 32.0.100.4841) *Try NPU
now* gave a verified answer with Qwen 2.5 3B at about 13 tokens a second, after about 45
seconds of loading; the same model ran at 26 tokens a second on that machine's plain CPU
engine and 122 on its NVIDIA card. OpenVINO's server also held 7 to 9 GB of memory there,
against 3 to 4.4 GB for the other engines. The figures and the rest of that run are in
[HANDOVER.md](HANDOVER.md). Other NPUs, drivers and models remain untried: treat yours as
unknown until *Try NPU now* reports a verified answer on it.

### Test performance

*Test performance* stops the chat server, then for each installed engine in turn: starts the
chat model on it alone, confirms the device from the load log, runs a short warm-up, and times
one fixed question (a few hundred tokens of prompt shaped like one of Loft's own, 96 tokens of
answer) with the same model file and context size. It records the time to load, the time to
the first token, tokens per second, the total time, the server's peak memory, the memory the
engine took on the device, whether it worked, and which device really ran it. Then it starts
the chat model again. It takes about half a minute per engine and never runs on its own.

- **How much to trust the numbers.** The same engine measured twice differs by about a tenth,
  and more if other programs are busy. A fast engine's question is therefore asked three times
  and the middle speed kept, and engines within 15% of the best are treated as equals: among
  them the rule-of-thumb order decides, and the recommendation says "measured about the same".
  On the development laptop CUDA and Vulkan on the same NVIDIA card came out at 73 and 65
  tokens a second in one run and 64 and 73 in the next.

- **Energy** is measured only while on battery, as the whole computer's drain as its battery
  reports it (screen and everything else included), divided by the speed: joules of battery
  per token. It compares engines; it does not predict battery life, and it is not the draw of
  one processor. Plugged in, or where the battery reports nothing, the result says
  *energy unmeasured* and no claim about efficiency is made.
- Results are stored in `benchmarks.json` with what they were measured on: the model file and
  its size, the context size, the llama.cpp revision, the processor, the device and its driver
  version. A result whose identity no longer matches (another model, a driver update, a new
  engine release) is shown as *out of date* and is not used.
- The recommendation is shown with the numbers behind it and a *Use it* button. Nothing is
  changed unless you press it.

## Reliability

- A model server that dies is restarted on the same engine (up to 3 times in 5 minutes), by
  the same coordinator that does every other switch, so the two cannot collide. Until it is
  back the status says it is not running.
- An answer that is cut off because its server went away is reported as an error, not shown
  as if it were complete.
- On Windows the servers are tied to Loft's process, so they cannot outlive it even if Loft is
  killed. On macOS/Linux they are stopped on a normal quit only.
- The window is only as large as what is drawn, including the margin its shadow needs (top
  and floating: pill 320×76, panel 540×580; right: pill 76×274, panel 520×600), so it does not
  block clicks on what is underneath.
- A floating window is kept wholly on a display: left near an edge, its panel opens moved in
  just far enough to fit, and if its display is unplugged it returns to the default spot.

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

`scripts/smoke-engines.js` does the same for Performance & battery, on real servers, models
and devices (it was written on a laptop with NVIDIA and Intel graphics and skips itself where
there are not two GPU targets): choosing each device through the Settings page and confirming
it from the server's log, CPU only, a separate embeddings device, the power profiles
(simulated: nobody pulls the charger), duplicate and flapping power notifications, a switch
that waits for an answer and one that waits for indexing, fallback on and off with *Retry*, a
temporary engine with its test answer, the NPU card, the optional-engine list and its guards,
crash recovery, and the performance test. After every step it counts the `llama-server`
processes, so a duplicate or an orphan fails the run.

```powershell
scripts\win-smoke.bat smoke-engines.js
```

`scripts/smoke-restart.js` is run after the UI test, on the profile it left behind
(`set LOFT_SMOKE_KEEP=1` first): Loft has been closed and started again, and the script checks
that the chat history is as it was saved, then opens a conversation from it and continues it.

`scripts/smoke-optional-engines.js` installs CUDA and OpenVINO from inside the app (it
**downloads about 700 MB** unless they are already there), tries every device of both for
real, checks the no-fallback stop and the removal guard, and measures everything. It asserts
only what must always hold and reports the rest as it happened. Point it at a test folder so
the engines do not land in your own Loft:

```powershell
set LOFT_ENGINES_DIR=%TEMP%\loft-smoke-engines
scripts\win-smoke.bat smoke-optional-engines.js
```

All three tests run in a throwaway profile (`%TEMP%\loft-smoke-profile`) whose only folder is
`sample-docs`: your own folders, index and settings are neither read nor changed. Two more
debug-build switches make that possible: `LOFT_DATA_DIR` puts the index and settings in
another folder (models are still read from the real one), and `LOFT_SEED_FOLDER` registers a
folder at launch. The picture-descriptions step needs the picture model to be downloaded.
Debug builds also have `dev_set_power` (pretend the power source changed), `dev_fail_target`
(make one engine fail to start), `dev_kill_engine` (kill a server as a crash would) and
`dev_engines` (the servers' process ids); release builds refuse all four.

## Known gaps

[HANDOVER.md](HANDOVER.md) sets out what was verified on which system, what was not, and how
to check the rest on other hardware and operating systems.

- **macOS and Linux are untested.** The code paths, `fetch-llama` entries and bundle config
  exist, but nothing has been built or run there. Window resizing of a non-resizable window
  and the tray icon are the likeliest things to need attention.
- **Not exercised by any test:** the native Add-folder dialog, the remove-folder button,
  clicking a source to open it, the tray icon and the global hotkey (they need a real click or
  key press), turning on Launch at login (only allowed in an installed build), and installing
  from the installers (they were built and their contents checked, not run).
- **The NPU has been run on one machine, with one model.** See
  [OpenVINO and the NPU](#openvino-and-the-npu-what-experimental-means-here).
- **CUDA was run on one laptop, OpenVINO on two.** `fetch-llama cuda` /
  `fetch-llama openvino` share the rules of the in-app installer but have not themselves been
  run for those two engines.
- **The relevance floor is measured for two embedding models only.** Each embedding model
  scores on its own scale, so each has its own floor in `catalog.json` (`minScore`): 0.45 for
  Nomic Embed Text and 0.20 for EmbeddingGemma, both measured on the sample folder. (With one
  floor for both, EmbeddingGemma found nothing: it scores a relevant excerpt around 0.25 to
  0.35.) **BGE-M3 has not been measured** and takes the default of 0.45, which may be wrong
  for it. Other folders may sit differently from the sample one.
- **Dragging the floating window, the tray icon and the hotkey need a real hand.** The test
  moves the window and calls what the tray icon calls; nobody has yet dragged it with a mouse
  or clicked the icon on macOS or Linux.
- **Power changes were simulated.** Windows' power notification is registered and the
  switching logic is tested end to end with simulated readings, but the charger was not
  actually pulled, and no energy figure has been measured (that needs a run on battery).
- **Chat history has only been tested with dozens of conversations**, not thousands; search
  is a plain scan of titles and messages, with no full-text index.
- **Per-model engine choices are not implemented**: the plugged-in and battery choices apply
  to whichever chat model is selected.
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
  src/lib.rs              # commands, window sizing, tray, hotkey
  src/inference.rs        # engines and devices: the decision rules (no processes; unit-tested)
  src/engine.rs           # the coordinator: every start, switch and restart of a server
  src/runtime.rs          # finding the engines, starting and stopping llama-server
  src/hardware.rs, power.rs   # what the OS says: GPUs, NPUs, drivers; mains or battery
  src/installer.rs        # optional engines: pinned download, checksum, safe unpacking
  src/bench.rs            # Test performance, Try … now, and what the measurements suggest
  src/history.rs          # chat history: conversations, messages and their sources (chats.db)
  src/models.rs           # model downloads
  src/store.rs            # sqlite-vec store
  src/parse.rs, chunk.rs, indexer.rs, jobs.rs   # indexing + file watching
  src/ocr.rs              # text recognition for pictures and scanned PDF pages (Windows)
  src/vision.rs           # optional picture descriptions from a small vision model
  src/rag.rs, opener.rs   # answers + opening sources
  catalog.json            # curated model catalog (GGUF URLs)
  engines.json            # the llama.cpp builds of one pinned release, with checksums
  tauri.conf.json         # transparent, top-center, always-on-top window; bundle config
scripts/
  fetch-llama.mjs         # download + verify the llama.cpp builds for this OS
  make-engines-manifest.py   # re-pin engines.json to another llama.cpp release
  smoke-ui.js             # drives the real UI in a debug build (see Testing)
  smoke-engines.js        # the same for engines, power, fallback and the performance test
  smoke-restart.js        # after smoke-ui.js: the chat history survived closing Loft
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
