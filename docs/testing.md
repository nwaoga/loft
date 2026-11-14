# Testing

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
