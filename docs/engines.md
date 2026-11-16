# Engines, performance and battery

An **engine** is a llama.cpp build (the software path): CUDA, Vulkan, OpenVINO, Metal or CPU.
A **device** is the processor that does the work: an NVIDIA card, the Intel graphics built
into the processor, an Intel NPU, the CPU. Loft keeps the two apart, because one engine can
drive several devices (Vulkan: any GPU) and one device can be driven by several engines (an
NVIDIA card: CUDA or Vulkan).

**Settings → Performance & battery** is where you choose. Loft recommends; you decide.

**Larger window** (the arrows icon at the top of the panel, always there) doubles the panel,
to 920×1040, to see more at once; on a display without room for that it grows as far as
fits, never smaller than normal. Press it again to go back. Loft remembers the choice.

**Usage** (the pulse icon next to the gear) shows what Loft is using right now, updated every
2 seconds while it is open: bars for memory (Loft's share of the computer's, with other
programs alongside), the graphics card's memory and the processor, then each part (chat
model, embedding model, the picture model while it reads pictures, Loft and its window) with
its memory, graphics memory, processor share, model size and context.

| Control | What it does |
|---|---|
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

## Engines

| Engine | Devices | How it gets here |
|---|---|---|
| CPU | the processor | `npm run fetch-llama` (bundled in the installer) |
| Vulkan | any GPU, each selectable by name | `npm run fetch-llama` (bundled) |
| Metal | Apple silicon | `npm run fetch-llama` on a Mac (bundled) |
| CUDA | NVIDIA cards | optional, about 590 MB: *Install* in Settings, or `npm run fetch-llama cuda` |
| OpenVINO (experimental) | Intel processors, graphics and NPUs | optional, about 105 MB: *Install* in Settings, or `npm run fetch-llama openvino` |

All of them are official prebuilt binaries of **one** llama.cpp release (`b12110`), pinned in
[`src-tauri/engines.json`](../src-tauri/engines.json) together with the size and SHA-256 that
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

## OpenVINO and the NPU: what "experimental" means here

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
[HANDOVER.md](../HANDOVER.md). Other NPUs, drivers and models remain untried: treat yours as
unknown until *Try NPU now* reports a verified answer on it.

## Test performance

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
