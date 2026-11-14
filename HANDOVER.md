# Loft — verification handover

This document records what Loft does, what was actually tested on two Windows 11 laptops,
what remains unverified, how to run the tests, and where to look when something differs.

Read [README.md](README.md) for what Loft is and how it is used, and
[ARCHITECTURE.md](ARCHITECTURE.md) for how it is put together. This file does not repeat them.

## 1. What is in this version

| Piece | What it does | Main files |
|---|---|---|
| **Performance, battery and engines** | A choice of engine (llama.cpp build) and device for mains and for battery; automatic switching on a power change; fallback on or off; "Try another engine now" and "Try NPU now"; optional engines (CUDA, OpenVINO) installed from Settings with pinned checksums; "Test performance" with recommendations. What is shown as running is read from the model server's own log. | `src-tauri/src/{inference,engine,runtime,hardware,power,installer,bench}.rs`, `src-tauri/engines.json`, `src/lib/components/{Performance,Benchmark}.svelte`, `scripts/{fetch-llama.mjs,make-engines-manifest.py}` |
| **Window style and tray** | Notch (docked at an edge) or Floating (a window that stays where it is dragged); "Live in the tray" (nothing on screen until opened from the tray icon or the hotkey). | `src-tauri/src/lib.rs` (`Style`, `place_floating`, `set_tray_only`), `src/lib/window-style.svelte.ts`, `src/App.svelte`, `src/lib/components/Settings.svelte` |
| **Chat history** | Conversations saved in their own SQLite file, with each answer's sources; a history panel (search, resume, rename, delete); answers that did not finish are stored as such; saved history is separate from what the model is sent. | `src-tauri/src/history.rs`, `src-tauri/src/lib.rs` (`ask`, the `*_conversation` commands), `src/lib/components/{History,Ask}.svelte` |

Smaller changes that came out of testing, worth knowing about:

- While the models are on their way (at launch, during a switch, during a performance test)
  the panel says so, instead of "No models selected".
- An answer whose server goes away mid-stream is an error, not a short answer.
- Model servers are started in a scratch folder (`<temp>/loft-engine-work`), because engines
  write stray files into their working directory.
- Choices are named device first: `NVIDIA GeForce RTX 4050 Laptop GPU (CUDA)`.
- Added 13 November 2026: **huge files are indexed in part.** A real work folder (a OneDrive
  with sales spreadsheets and JSON menu exports) spent many minutes on one file and looked
  stalled: one 6 MB spreadsheet became 40,017 chunks. Now at most 2,000 chunks per file
  are indexed, the folder says how many files were cut short, progress shows chunks within a
  file, and Cancel works in the middle of a file. Tested with a 100,000-row spreadsheet
  (`indexer::tests::a_huge_file_is_capped`, real embedding model).
- Added 13 November 2026: **Larger window**, a switch at the top of the panel that doubles
  it (920×1040, or as much as the display's usable area allows). Checked here in the
  browser preview only; the UI test now checks the real window grows and shrinks with it,
  but has not been run. Worth checking with the notch at the top and on the right edge, in
  floating style, and on a small or high-scaling display.
- Added 13 November 2026: **Usage**, a live view opened from the pulse icon next to the gear
  (bars for memory, graphics-card memory and processor, then each part of Loft). It
  found that a model on a graphics card was also held in RAM, as a memory-mapped copy of its
  file. Servers on a card with memory of its own (CUDA, Vulkan) now start with
  `--load-mode none`: the chat server on the RTX 4050 went from 2.3 GB to 0.64 GB of RAM.
  Worth checking on your machines: the panel's figures against Task Manager (Details tab,
  "Working set"), that CUDA and Vulkan on the RTX 5070 Ti still load and answer, and what
  OpenVINO on the NPU holds in RAM (the load mode was not changed there). On Linux and macOS
  the panel's process code has not been run.
- Added 13 November 2026: small talk ("hi", "thanks", "who are you?") gets a short friendly
  reply instead of the refusal, and the refusal now ends with the user's folder names: "I
  couldn't find that in your indexed files. I'm only really here to talk about items in
  sample-docs." See "How answers work" in [docs/how-it-works.md](docs/how-it-works.md). Checked with Qwen 2.5 3B only; whether
  other chat models repeat the longer refusal word for word is not known.

## 2. What was verified, and where

The development machine: Windows 11, Intel Core i7-12700H, Intel Iris Xe graphics, NVIDIA RTX
4050 Laptop GPU (6 GB), 32 GB RAM, a battery, **no NPU**, two displays. Chat model Qwen 2.5 3B
(Q4_K_M), embedding model Nomic Embed Text v1.5. llama.cpp `b12110`.

| Check | Result on that machine |
|---|---|
| `cargo test --lib` | 95 passed, 12 ignored (the ignored ones need downloaded models) |
| `rag::tests::end_to_end`; `indexer::tests::end_to_end` (real models) | passed |
| `runtime::tests::each_target_runs_where_it_is_told` | passed: CPU only, Vulkan on the Intel graphics, Vulkan on the NVIDIA card, each confirmed from the server's log |
| `scripts\win-smoke.bat` (UI test, about 60 steps) | passed, including floating style, tray mode and chat history; also passed with EmbeddingGemma as the embedding model |
| `scripts\win-smoke.bat smoke-restart.js` | passed: history intact after closing and reopening Loft |
| `scripts\win-smoke.bat smoke-engines.js` (16 scenarios) | passed on the final build |
| `scripts\win-smoke.bat smoke-optional-engines.js` | passed: CUDA and OpenVINO installed from Settings (real downloads, checksums verified) and every device of both tried |
| Release build (`scripts\win-build.bat`) | builds without warnings; the built app was run by hand |
| `svelte-check` | 0 errors, 0 warnings |

Two last-minute changes were only compile- or syntax-checked, not run: the non-Windows branch
of `dev_engines` (`pgrep`), and the UI test skipping its two OCR steps off Windows.

What the real optional engines did there (details in [docs/engines.md](docs/engines.md)):

- **CUDA** works; on the same NVIDIA card it measures the same as Vulkan (64 to 73 tokens/s
  each, trading places between runs) and loads the model about twice as fast.
- **OpenVINO** works on the Intel processor and Intel graphics, slowly; it lists the NVIDIA
  card but dies at start on it. Its `--list-devices` prints nothing on this build, so Loft
  reads its devices from the engine's refusal of a non-existent device.

### Second Windows machine — 12 November 2026

Windows 11, Intel Core Ultra 9 386H, Intel Graphics, NVIDIA GeForce RTX 5070 Ti Laptop GPU,
Intel AI Boost (NPU 5010, driver 32.0.100.4841), 31.5 GB RAM, one display. The real-model
tests used Qwen 2.5 3B Instruct Q4_K_M and Nomic Embed Text v1.5 F16, both verified against
the catalog hashes. The user's normal Llama 3.2 3B / EmbeddingGemma selection was left alone;
the smoke tests used a separate profile under `%TEMP%\loft-smoke-profile`.

| Check | Result on this machine |
|---|---|
| `npm run check`; `npm run build` | passed; Svelte reported 0 errors and 0 warnings |
| `cargo test --lib` | 84 passed, 10 ignored |
| `models::tests::installed_models_match_catalog` | passed for all four installed chat/embedding model files |
| `runtime::tests::engines_on_this_computer_are_discovered` | passed; Intel NPU and both GPUs detected; NVIDIA Vulkan chosen automatically |
| `rag::tests::end_to_end`; `indexer::tests::end_to_end` | passed with Qwen/Nomic; the indexing test's expected counts were updated to include a tracked picture with no OCR text |
| `runtime::tests::each_target_runs_where_it_is_told` | passed: CPU only, Intel Vulkan and NVIDIA Vulkan each confirmed from the server log |
| `scripts\win-smoke.bat` | `[dev] 17 DONE`; the test now waits for the sample folder to be searchable before asking |
| `scripts\win-smoke.bat smoke-restart.js` | `[dev] R-DONE`; saved history, sources, usage and stopped answers survived restart |
| `scripts\win-smoke.bat smoke-engines.js` | `[dev] E-DONE`; NPU trial verified a real answer. The performance assertion now records explained OpenVINO failures without failing standard engines. |
| `cargo fmt --check` | fails on widespread pre-existing Rust formatting differences; no repository-wide reformat was made |

The plugged-in performance test used the same Qwen file and prompt for every target. These
figures are one run, not a cross-machine ranking:

| Target | Tokens/s | Load | Peak server working set |
|---|---:|---:|---:|
| NVIDIA Vulkan | 121.9 | 2.3 s | 3,004 MiB |
| Intel Vulkan | 31.1 | 1.6 s | 3,504 MiB |
| CPU only | 26.0 | 2.4 s | 4,398 MiB |
| Intel Graphics OpenVINO | 16.3 | 10.4 s | 8,663 MiB |
| Intel NPU OpenVINO | 13.7 | 44.7 s | 7,152 MiB |
| Intel CPU OpenVINO | 9.6 | 12.1 s | 9,097 MiB |
| NVIDIA OpenVINO | failed to load; Loft explained to use CUDA or Vulkan | — | — |

*Try NPU now* also answered on the Intel NPU at about 13 tokens/s, with the device confirmed
from OpenVINO's log. During an earlier Llama 3.2 NPU trial, Windows showed about 30 GB of
31.5 GB RAM in use system-wide; the OpenVINO server's working set was 8.3 GB at inspection.
The 30 GB figure must not be attributed to the NPU server alone. Energy was not measured:
the laptop stayed plugged in. CUDA, the optional-engine installer smoke test, and the picture
description model were not exercised on this machine.

**Retrieval issue found here, since fixed:** With the user's Llama 3.2 / EmbeddingGemma pair,
both smoke scripts stopped at their first sourced-answer check. The seven sample files and
vectors were indexed, but the supplier-contract question returned "I couldn't find that in
your indexed files" with no sources, including after the index-ready wait. The same scripts
passed with Qwen/Nomic.

The cause, measured afterwards on the first machine (13 November 2026): the floor below which
nothing counts as relevant was one number, 0.45, measured with Nomic. EmbeddingGemma scores on
a different scale. Best match per question on the sample folder's text files:

| Embedding model | Unrelated questions | Answerable questions |
|---|---|---|
| Nomic Embed Text v1.5 | 0.47 to 0.58 | 0.57 to 0.84 |
| EmbeddingGemma 300M | 0.00 to 0.19 | 0.25 to 0.75 |

"Who owns the supplier contract?" scored 0.25 with EmbeddingGemma, so it was discarded before
the chat model was asked. Each embedding model now has its own floor in `catalog.json`
(`minScore`: 0.45 for Nomic, 0.20 for EmbeddingGemma). On the first machine `scripts\win-smoke.bat`
then reached `[dev] 17 DONE` with Qwen 2.5 3B and EmbeddingGemma, and again with Qwen and
Nomic. **Still to check:** the Llama 3.2 / EmbeddingGemma pair itself on this machine (the
re-test used Qwen as the chat model), and BGE-M3, which has no measured floor.

### GitHub-hosted macOS 15, Apple Silicon — 14 November 2026

Not a laptop on a desk. A `macos-15` Actions runner: macOS 15.8.2, arm64, 3 CPU threads,
display 1024×768 at scale 1 (not Retina; the menu bar is 25 points), GPU reported by
llama.cpp as `MTL0: Apple Paravirtual device (4778 MiB)`. Chat model Qwen2.5 3B Instruct
Q4_K_M and embedding model Nomic Embed Text v1.5 F16, both checked against the catalog
hashes. The workflow is `.github/workflows/macos.yml`. `macos-14` had already been retired,
so that label never started a machine.

| Check | Result |
|---|---|
| `npm run check` | passed |
| `cargo test --lib` | 94 passed, 12 ignored |
| `npm run fetch-llama` | Metal archive unpacked (30 files). Symlinked `.dylib` names are copied as real files; Node's `cpSync` refuses them on macOS |
| `llama-server --version` and `--list-devices` | `0.9.0-dev (build 12110)`; `MTL0` listed |
| `engines_on_this_computer_are_discovered` | `Metal Available`; CPU build not installed (none is pinned for `mac-arm64`); default target **Metal** |
| Promo script on `sample-docs` | indexed 7 files, 4 chunks; answer on Metal: "Priya owns the supplier contract with BeanWorks." at 2 tokens/s, one source (`project-plan.md`, 67%) |
| Window resize (non-resizable window) | collapsed 320×76 at (352, 25); expanded 540×580 at (242, 25); larger 1000×699; right edge 520×600 at (504, 84), flush with the 1024-wide screen and centred vertically |
| Tray | `trayAvailable=true`. A purple menu-bar icon is visible in the screenshots. AppleScript did not find a status item named "Loft", so the menu was not opened |
| Global hotkey | registration logged no error on the runner. The key was not pressed there, and no Accessibility prompt was observed. The owner has since pressed Cmd+Shift+Space on a real Retina Mac and it summoned Loft |
| `smoke-engines.js` | `E-SKIP` (one GPU target, as the script requires two) |
| `npm run tauri build` | `Loft.app` 81.64 MiB, ad-hoc signed; `Loft_0.1.1_aarch64.dmg` 29.49 MiB. The bundled `llama-server` still reports build 12110 |
| Launch the built app | `open Loft.app` left the process running; the collapsed notch was on screen |

Two fixes came out of the first red runs. `fetch-llama` used `cpSync` on symlinked libraries, which macOS Node rejects. And Metal is not asked for a device list, so resolving it used to demand a graphics card, find none, and refuse to start a model. It now runs as the Apple GPU with no `--device` argument.

**Not verified on macOS:** the runner is not a Retina display, and on that runner the hotkey was not pressed and the tray menu was not opened. The owner has pressed Cmd+Shift+Space on a real Retina Mac and it summoned Loft. Still untested: opening the tray menu, dragging the floating window, `smoke-ui.js` and `smoke-restart.js` in full, OCR, power switching (still always "unknown", treated as mains), the hardware list (still empty), and running the `.dmg` through an install.

### Linux x64 under Xvfb — 14 November 2026

Not a desktop someone sits at. The same arrangement as `.github/workflows/linux.yml`:
Ubuntu, x86_64, Xvfb at 1920×1080, xfwm4 (GTK will not resize the notch without a window
manager), picom, and an xfce panel with the StatusNotifier plugin for the Ayatana
indicator. `GDK_SCALE=2` on a 3840×2160 screen was tried and dropped: GTK sized the window
at 2×, but WebKit kept `devicePixelRatio` at 1, so the notch sat in the top half of a
window twice as tall as the panel. Chat model Qwen2.5 3B Instruct Q4_K_M and embedding
model Nomic Embed Text v1.5 F16, both checked against the catalog hashes. The CPU reports
itself as `Intel(R) Xeon(R) Processor` and has AMX.

| Check | Result |
|---|---|
| `npm run check` | 0 errors, 0 warnings |
| `cargo test --lib` | 94 passed, 12 ignored |
| `npm run fetch-llama` | CPU archive (33 files) and Vulkan archive (34 files). Both `llama-server` binaries run and report `0.9.0-dev (build 12110)` |
| Vulkan devices | `--list-devices` prints none. With `GGML_VK_VISIBLE_DEVICES=0` it lists `llvmpipe` (the software renderer). Loft does not set that variable, so Vulkan stays "installed but no device" and the default target is **CPU only** |
| `engines_on_this_computer_are_discovered` | `CPU Available`. Vulkan is `HardwareUnavailable` (no device). That used to fail the test, because `fetch-llama` installs the Vulkan build on every Linux machine |
| Promo script | indexed the sample notes (a larger copy, 37 files / 34 chunks, so the indexing screen stayed up long enough to photograph; the plain `sample-docs` folder is 7 files / 4 chunks). Answer on CPU: "Priya owns the supplier contract with BeanWorks." one source (`project-plan.md`) |
| Window resize | collapsed 320×76 at (800, 0); expanded 540×580 at (690, 0); larger 1000×996; right edge 520×600 at (1400, 240), flush with the 1920-wide screen and centred vertically. GTK ignores a shrink of a window that was created non-resizable, and the first configure event locks that flag again; the size that sticks is the one set as both minimum and maximum, applied again after the window is shown |
| Tray | `trayAvailable=true`. The Loft icon is drawn in the panel by the Ayatana indicator. The menu was not opened |
| Global hotkey | registration logged no error. `xdotool key ctrl+shift+space` (XTEST, so the grab sees it) opened the panel |
| CPU load log | the weights were reported in an `AMX` buffer. That is the CPU build using Intel AMX, still in system memory; it was being rejected as an offload onto another device |
| `npm run tauri build` | `Loft_0.1.1_amd64.deb` 73.32 MiB and `Loft_0.1.1_amd64.AppImage` 105.35 MiB. The bundled `llama-server` reports build 12110 |
| Launch the built app | the binary extracted from the deb opened a 320×76 window at (800, 0). It was not installed with `dpkg` |

**Not verified on Linux:** a real window manager session someone is using (this was Xvfb), Wayland (an app still cannot place its own window there), a real GPU so Vulkan would be chosen, opening the indicator's menu, dragging the floating window, `smoke-ui.js` and `smoke-restart.js` in full, OCR, the hardware list (still empty), and installing the `.deb` or the AppImage through a package manager and launching that install. How to build those packages is in [docs/building.md](docs/building.md).

## 3. What remains unverified

This is the list to work through beyond the two Windows machines, the macOS runner, and the Linux Xvfb run above.

**Hardware and power**

- **Other NPUs and drivers.** Qwen 2.5 3B ran on one Intel NPU 5010 as recorded above;
  compatibility and memory use on other hardware remain unknown.
- **AMD graphics**, a machine with **one GPU**, a machine with **none**, **Windows on ARM**.
- **Really running on battery.** Power changes were simulated (`dev_set_power`). The Windows
  power notification is registered but the charger was never pulled, and no energy figure
  has been measured (that needs *Test performance* run on battery).

**Linux has been built and run once, under Xvfb, as recorded above. macOS has, once, on the runner above, and the owner has pressed the Mac hotkey on a Retina display.**

Known differences, by design or by omission:

| Area | Windows | macOS | Linux |
|---|---|---|---|
| Engines pinned in `engines.json` | cpu, vulkan, cuda, openvino | arm64: metal only; x64: cpu only | cpu, vulkan, cuda (x64), openvino (x64) |
| Power source (`power.rs`) | notifications from the power manager | **not implemented: always "unknown", treated as mains, so the battery choice is never used** | `/sys/class/power_supply`, polled every 30 s |
| Hardware list (`hardware.rs`) | SetupAPI: GPUs, NPUs, driver versions | empty | empty |
| Peak memory in the performance test | yes | no | `/proc/<pid>/status` |
| Model servers die with Loft if it is killed | yes (job object) | no: stopped on a normal quit only | no |
| OCR for pictures and scanned PDFs | yes | no text is indexed for them | no |
| Optional engine archives | `.zip`, unpacked in-process | `.tar.gz` via the system `tar` | `.tar.gz` via the system `tar` |
| Tray | left click opens, menu has Show/hide and Quit | menu bar icon was created and is visible on the runner; the menu was not opened. "Live in the tray" also drops the Dock icon **from the next launch** (not tried) | with `libayatana-appindicator3` and a StatusNotifier host, the icon was drawn in the panel under Xvfb. The menu was not opened. Click events may not arrive (use the menu) |
| Window placement | works | notch, expanded panel, larger panel, and the right edge were measured on the 1024×768 runner. Not Retina, and not dragged with a mouse | notch, expanded panel, larger panel, and the right edge were measured under Xvfb at 1920×1080 with xfwm4. **Wayland does not let an app position its window**, so docking and the saved floating position are in doubt there |

**Interaction that needs a real hand**

- Dragging the floating window with a mouse (the test moves it programmatically).
- Clicking the tray icon. The Mac hotkey Cmd+Shift+Space was pressed by the owner on a
  real Retina Mac and summoned Loft. On Linux, Ctrl+Shift+Space was delivered with
  `xdotool` under Xvfb and opened the panel; that is not a finger on a keyboard.
- The native Add-folder dialog; clicking a source to open it.

**Other**

- `node scripts/fetch-llama.mjs cuda` / `openvino`: same rules as the in-app installer, but
  not themselves run for those two engines.
- Chat history with thousands of conversations (search is a plain scan), and the one-time
  import of the conversation an older build kept in local storage.
- Per-model engine choices: not implemented.

## 4. Setting up on another machine

Prerequisites are in [docs/building.md](docs/building.md). In short:

```bash
npm install
npm run fetch-llama                 # the standard engines for this OS, into src-tauri/binaries/
npm run tauri dev                   # macOS / Linux
```

On Windows use `scripts\win-dev.bat` instead of the last line (it selects the MSVC toolset).

Then, in the running app: Settings, download and select **Qwen2.5 3B Instruct** and **Nomic
Embed Text v1.5**. The tests' expected answers were tuned on that pair; another chat model
may phrase things differently and fail a check that is about wording, not behaviour.

`src-tauri/binaries/`, `src-tauri/gen/`, `node_modules/`, `dist/` and `releases/` are not in
the repository. The icons now are (`src-tauri/icons/`, made from `app-icon.svg`).

To make an installer: `scripts\windows\release.bat` (see [docs/building.md](docs/building.md)).
Worth checking on another Windows machine: that the installer from
`releases\windows\<version>\` installs, starts, shows its version at the bottom of Settings,
and that a newer one installs over it keeping models and folders.

## 5. Running the tests

### Unit and real-model tests

```bash
cd src-tauri
cargo test --lib
cargo test --lib rag::tests::end_to_end -- --ignored --nocapture
cargo test --lib runtime::tests::each_target_runs_where_it_is_told -- --ignored --nocapture
cargo test --lib runtime::tests::engines_on_this_computer -- --nocapture
```

The last one prints what Loft finds on the machine (CPU, hardware, each engine and its
devices, the default target). Its output is the first thing to look at on a new system.

### Real-app tests

A debug build runs the script named by `LOFT_DEV_EVAL` inside its own webview once the
models are up, and the script reports through the `dev_log` command. Each step prints as
`[dev] ...` in the terminal, with the window's size and position.

| Script | What it covers | Ends with | Takes |
|---|---|---|---|
| `smoke-ui.js` | Ask, sources, follow-ups, chat options, chat history, stop, tabs, folders, themes, placement, displays, floating, tray, quit | `[dev] 17 DONE` | about 8 min |
| `smoke-restart.js` | Run after `smoke-ui.js` with the profile kept: the history survived a restart | `[dev] R-DONE` | about 1 min |
| `smoke-engines.js` | Devices, CPU only, separate embeddings device, power profiles, pending switches, fallback, temporary engine, crash recovery, NPU card, engine list, performance test | `[dev] E-DONE` (or `E-SKIP`) | about 12 min |
| `smoke-optional-engines.js` | Installs CUDA and OpenVINO in-app and tries every device. **Downloads about 700 MB** | `[dev] I-DONE` | 15 min and up |

Any failure prints `[dev] FAILED: <reason>` and quits the app.
The Windows batch runner can still exit with code 0 after that marker, so check for `17 DONE`,
`R-DONE`, or `E-DONE` in the output rather than relying on its exit code alone.

Windows:

```powershell
scripts\win-smoke.bat
set LOFT_SMOKE_KEEP=1
scripts\win-smoke.bat smoke-restart.js
set LOFT_SMOKE_KEEP=
scripts\win-smoke.bat smoke-engines.js
```

macOS / Linux (`scripts/smoke.sh` mirrors `win-smoke.bat`. On 14 November 2026 it was run on the macOS 15 runner for the promo script, the window probe, and `smoke-engines.js`, and on Linux under Xvfb for the promo script and the window probe):

```bash
scripts/smoke.sh
LOFT_SMOKE_KEEP=1 scripts/smoke.sh smoke-restart.js
scripts/smoke.sh smoke-engines.js
```

Things to know before running them:

- **They use a throwaway profile**, `%TEMP%\loft-smoke-profile` or `/tmp/loft-smoke-profile`:
  its own index, settings and chat history, with `sample-docs` as the only folder. Your own
  folders, settings and conversations are not read or changed. The models are read from the
  real app-data folder, and the chat and embedding model you have selected are used.
- **Optional engines are read from the real app-data folder** (`<app data>/engines`) unless
  `LOFT_ENGINES_DIR` points elsewhere. Set it for `smoke-optional-engines.js`, or the engines
  it downloads land in your own Loft.
- **Do not use the test window.** It sits where Loft normally is and says "TEST RUN, not your
  Loft". A question typed into it stops the answer the test is waiting for.
  `smoke-engines.js` recognises that in its longest scenario (E8) and retries, but other
  steps do not.
- **Close any other running Loft first.** Two copies of the model will not both fit on a
  small graphics card, and the measurements would mean nothing.
- **Other busy programs skew the performance figures**, the CPU engine's most of all (2 to 11
  tokens/s was seen for the same engine depending on what else was running).
- `smoke-engines.js` needs **two GPU targets** (it was written on a laptop with Intel and
  NVIDIA graphics) and prints `E-SKIP` otherwise. It looks for options whose names contain
  "NVIDIA" and "Intel". On other hardware its scenarios have to be adapted, or walked
  through by hand from the script's comments.
- The UI test's display steps assume the main display is where the notch starts; its
  picture-description steps run only if the picture model is downloaded; its OCR steps are
  skipped off Windows.
- Debug builds have extra commands the tests use: `dev_set_power` (pretend the power source
  changed), `dev_fail_target` (make one engine fail), `dev_kill_engine`, `dev_engines`
  (process ids, and how many questions and stops there have been), `dev_summon` (what the
  tray icon and hotkey do). Release builds refuse them.
- **Do not edit files under `src-tauri/` while `tauri dev` is running a test**: the dev tool
  rebuilds and restarts the app.

## 6. Suggested order on a new system

1. Build and launch. Does the window appear where it should, at the right size, and can the
   panel be opened and closed? (On Linux, note the session type: X11 or Wayland.)
2. `cargo test --lib`, then `engines_on_this_computer -- --nocapture`. Is the hardware and
   engine list right for the machine?
3. Download and select the two models; ask a question about `sample-docs`.
4. Settings, Performance & battery: is what it shows as running true? Choose each device in
   turn; each should be "Confirmed from the engine's own log" or fail with a reason.
5. `smoke-ui.js`, then `smoke-restart.js`.
6. `smoke-engines.js` where there are two GPUs; otherwise by hand: CPU only, a temporary
   engine and Restore, fallback off with a failing choice, Test performance.
7. On a laptop: unplug it for real. Does the status change to "On battery" and, with
   switching on, does the battery choice take over once any answer in progress has finished?
   Then run Test performance on battery: is an energy figure reported?
8. Floating style: drag it with the mouse, close and reopen, quit and relaunch. Live in the
   tray: close the panel, bring it back from the tray icon and from the hotkey.
9. Where there is an NPU: install OpenVINO from Settings, *Try NPU now*.
10. Install an optional engine from Settings (it downloads): does the progress bar run, is
    the engine then listed with its devices, can it be removed again?

## 7. When something differs: where to look

| Symptom | Look at |
|---|---|
| An engine is "not installed" though it was fetched | `runtime::find_binary` and `platform_dir`: the folder must be `binaries/<platform>-<backend>/` |
| A device is missing or misnamed | `inference::parse_devices` (the engine's `--list-devices`), `runtime::openvino_devices` |
| "Confirmed from the engine's own log" never appears, or a working engine is rejected | `inference::LoadReport::read` and `inference::verify`: they match llama.cpp's log lines (`using device`, `model buffer size`, `OpenVINO: using device`) at log level 4. A different llama.cpp release may word them differently |
| The power source is wrong or never changes | `power.rs` (per-OS `imp` modules), `inference::PowerDebounce`, `engine::watch_power` |
| A switch happens during an answer, or never happens | `engine::begin` / `Busy` (who holds a guard), `inference::decide` |
| The window is in the wrong place or the wrong size | `lib.rs`: `notch_size`, `notch_origin`, `floating_origin`, `move_and_size`, `dock` |
| The tray option is disabled | `build_tray` failed at start-up (stderr says why); `tray_ok` |
| An optional engine will not install | `installer.rs`: `Manifest::trusted`, `download`, `unpack_zip` / `unpack_tar`, `keep` (which files are kept) |
| A conversation is missing, or an answer has the wrong status | `history.rs` (`begin_turn`, `finish_turn`, `mark_interrupted`), and `ask` in `lib.rs` |

Things that looked like bugs on the development machine and were not, or were environment:

- **Test processes that never exit.** An earlier version queried graphics adapters through
  DXGI; with Windows' OCR and PDF engines in the same process, the process then spun on exit
  for ever, one core each. Hardware is now listed through SetupAPI instead. If unit-test
  processes (`loft_lib-*`) are ever left running, kill them: they starve everything else.
- **OpenVINO leaves a crash entry in the Windows event log** once per engine or driver
  change: that is the refusal Loft uses to learn its devices (cached in `loft-devices.json`
  beside the engine).
- **CUDA and Vulkan swapping places** in the performance test: run-to-run variation of about
  a tenth. Engines within 15% of the best are deliberately not ranked against each other.

## 8. Reporting back

For each system, the useful record is: OS and version, CPU, GPUs, NPU if any, session type on
Linux; the output of `engines_on_this_computer`; which of the tests in section 5 passed and
the first `[dev] FAILED` line of any that did not; and for anything in section 3, whether it
now works, with what was seen rather than what was expected. In particular: **do not report
NPU inference as working unless *Try NPU now* showed a verified answer on real hardware.**

## 9. Repository notes

- This project is the whole repository. It used to be the `V2/` folder; the first version
  (V1, a Python prototype at the top of the repository) was removed on 13 November 2026 and
  remains in the git history.
- The pinned llama.cpp release is in `src-tauri/engines.json` (`tag`) and
  `runtime::LLAMA_REVISION`; both must agree (a unit test checks). To move to another
  release, change `TAG` in `scripts/make-engines-manifest.py`, run it
  (`python scripts/make-engines-manifest.py src-tauri/engines.json`; it reads the release's
  asset list from GitHub), update `LLAMA_REVISION`, fetch again, and re-check the log lines
  in section 7.
- Installers are unsigned. The version number is in `package.json` only; change it with
  `npm run version:set` or the release script, not by hand in several places.
