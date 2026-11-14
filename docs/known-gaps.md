# Known gaps

[HANDOVER.md](../HANDOVER.md) sets out what was verified on which system, what was not, and how
to check the rest on other hardware and operating systems.

- **macOS has been built and run once, on a GitHub `macos-15` Apple Silicon runner**
  (macOS 15.8.2, 14 November 2026). `macos-14` no longer exists as a runner image. That
  machine is not Retina (1024×768 at 1×) and its GPU is the paravirtual device llama.cpp
  calls `MTL0`. What that run showed, and what it did not, is in
  [HANDOVER.md](../HANDOVER.md). In short: `fetch-llama` and the Metal server work, a debug
  build answered from `sample-docs` on Metal with a citation, the non-resizable window
  resized (collapsed, expanded, larger, and the right edge), the tray icon was created and
  is visible in the menu bar, the hotkey registered with no error, and `tauri build`
  produced an ad-hoc-signed `.app` and `.dmg` that launched. **Not done on macOS:** pressing
  Cmd+Shift+Space, opening the tray menu, dragging the floating window, a real Retina
  display, battery or power switching (still unimplemented and always treated as mains),
  OCR, and the full `smoke-ui.js` / `smoke-restart.js` scripts.
- **Linux x64 has been built and run** (debug app, `.deb`, and AppImage). Commands and
  where the files land are in [Building](building.md). Still open: Wayland, a machine with a
  GPU, arm64, and installing those packages with a package manager.
- **Not exercised by any test:** the native Add-folder dialog, the remove-folder button,
  clicking a source to open it, the tray icon and the global hotkey (they need a real click or
  key press), turning on Launch at login (only allowed in an installed build), and installing
  from the installers (they were built and their contents checked, not run).
- **The NPU has been run on one machine, with one model.** See
  [OpenVINO and the NPU](engines.md#openvino-and-the-npu-what-experimental-means-here).
- **CUDA was run on one laptop, OpenVINO on two.** `fetch-llama cuda` /
  `fetch-llama openvino` share the rules of the in-app installer but have not themselves been
  run for those two engines.
- **The relevance floor is measured for two embedding models only.** Each embedding model
  scores on its own scale, so each has its own floor in `catalog.json` (`minScore`): 0.45 for
  Nomic Embed Text and 0.20 for EmbeddingGemma, both measured on the sample folder. (With one
  floor for both, EmbeddingGemma found nothing: it scores a relevant excerpt around 0.25 to
  0.35.) **BGE-M3 has not been measured** and takes the default of 0.45, which may be wrong
  for it. Other folders may sit differently from the sample one.
- **Dragging the floating window and clicking the tray icon need a real hand.**
  The tests resize and move the window themselves. On the macOS runner the tray icon shows
  in the menu bar; the hotkey registered, and nobody pressed it or opened the menu there.
  On Linux the indicator was drawn, and Ctrl+Shift+Space sent with `xdotool` opened the
  panel. Nobody has clicked the icon or dragged the window.
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
