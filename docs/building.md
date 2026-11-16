# Building

Windows, macOS, and Linux. Unsigned builds: see [First open](first-open.md) before you launch one.

Every target needs **Node.js 18+** and a current **Rust stable** from rustup. The lockfile
uses Rust edition 2024, so 1.85 or newer: an older `rustc` (1.83 was tried) stops while
resolving crates. Loft has **one version number**, in `package.json`. The app, the installer, and
**Settings** all show it. `npm run version:set` keeps `src-tauri/Cargo.toml` in step
(`0.2.0`, or `patch` / `minor` / `major`; numbers only, which is what the Windows MSI
format allows).

One-time, in this repo, on every target:

```bash
npm install
npm run fetch-llama
```

`fetch-llama` downloads the llama.cpp server builds for **this** OS into
`src-tauri/binaries/` (not committed). CUDA and OpenVINO are optional extras
(`npm run fetch-llama cuda` or `openvino`); they are not in the installers below.
Models are not bundled either. They are downloaded in the app.

A debug run (`tauri dev`) builds `src-tauri/target/debug/` and opens Loft. A release
build (`tauri build`) writes `src-tauri/target/release/` and the installers under
`src-tauri/target/release/bundle/`. These builds are **unsigned**. The first time you
open one on macOS or Windows, see [First open](first-open.md).

## Windows x64

This is the path the Windows laptops and `scripts\windows\release.bat` were written for.
Pinned engines also exist for Windows on ARM (`win-arm64` in `engines.json`); that
installer has not been built or run.

**Prerequisites.** The MSVC C++ build tools and the Windows 10/11 SDK (Rust links with
them), and the WebView2 runtime (already on Windows 11).

**One time:**

```powershell
winget install --id Rustlang.Rustup -e
winget install --id Microsoft.VisualStudio.2022.BuildTools -e --override "--quiet --wait --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"
```

Open a new terminal so `PATH` is updated, then `rustc --version`, `npm install`, and
`npm run fetch-llama` (CPU and Vulkan).

**Debug run.** `scripts\win-dev.bat` selects the MSVC toolset and runs `npm run tauri dev`.
The debug binary is `src-tauri\target\debug\loft.exe`.

**Release.** `scripts\win-build.bat` is the optimised build and its installer, left under
`src-tauri\target\release\bundle\`. `scripts\windows\release.bat` also sets the version,
runs the type check and the unit tests, and copies a versioned set to
`releases\windows\<version>\`:

```bat
scripts\windows\release.bat 0.2.0
scripts\windows\release.bat patch
scripts\windows\release.bat
```

| File | What it is |
|---|---|
| `Loft_<version>_x64-setup.exe` | Per-user installer (no administrator) into `%LOCALAPPDATA%\Loft`, Start menu, and Installed apps. A newer installer replaces the older one. Settings, models, folders, and chats stay in `%APPDATA%\com.loft.desktop`. |
| `SHA256SUMS.txt` | The installer's checksum. |
| `RELEASE.txt` | Version, build date, llama.cpp release, which engines are inside. |

`-Msi` also writes an `.msi`. `-SkipTests` skips the checks. `-Force` remakes a version
that was already released; otherwise the script refuses, so a version number means one
build. The version stays changed if a later step fails: run it again once fixed. CPU and
Vulkan from `src-tauri/binaries/` are inside the installer.

**Signing.** Unsigned. SmartScreen shows "Windows protected your PC". See
[First open](first-open.md).

## macOS

**Apple Silicon is the verified target.** The `macos-15` CI job (arm64) fetches the Metal
engine, and `npm run tauri build` produces an ad-hoc-signed `Loft.app` and
`Loft_<version>_aarch64.dmg`. There is no Metal build for Intel. **Intel Macs** have a
pinned CPU engine (`mac-x64` in `engines.json`) and have not been built or run.

**Prerequisites.** Xcode command-line tools, Node.js, and Rust.

**One time:** `npm install`, then `npm run fetch-llama` (Metal on Apple Silicon, CPU on Intel).

**Debug run:**

```bash
npm run tauri dev
```

**Release:**

```bash
npm run tauri build
```

The app is `src-tauri/target/release/bundle/macos/Loft.app`. The disk image is
`src-tauri/target/release/bundle/dmg/Loft_<version>_aarch64.dmg` on Apple Silicon.
There is no `scripts/macos/` release script yet; CI stops at that bundle. The bundled
`llama-server` on the runner was the Metal build, still reporting llama.cpp `b12110`.

**Signing.** Ad-hoc, not an Apple Developer ID. Gatekeeper may say the app is from an
unidentified developer, or that it is damaged. See
[First open](first-open.md).

## Linux

**x64 is the verified target.** On Ubuntu 24.04, `npm run tauri -- build --bundles deb,appimage`
produced `Loft_0.1.1_amd64.deb` (73.32 MiB) and `Loft_0.1.1_amd64.AppImage` (105.35 MiB).
The binary taken out of the deb opened. It was not installed with `dpkg`. **arm64** has
pinned CPU and Vulkan engines (`linux-arm64` in `engines.json`) and has not been built or run.

**Prerequisites** (Ubuntu 24.04 package names; this is the set that compiled and bundled):

```bash
sudo apt install libwebkit2gtk-4.1-dev libayatana-appindicator3-dev \
  librsvg2-dev patchelf libssl-dev build-essential curl wget file \
  libxdo-dev pkg-config libfuse2t64
```

WebKitGTK 4.1 is the window. `libayatana-appindicator3` is the tray icon (without it the
tray option is disabled). `libfuse2t64` is what an AppImage needs to run; on Ubuntu 22.04
that package is `libfuse2`. The Tauri CLI downloads its own `linuxdeploy` when it builds
the AppImage.

**One time:** `npm install`, then `npm run fetch-llama` (CPU and Vulkan on x64). Vulkan
lists a device only when a GPU driver provides one. A machine whose only Vulkan device is
the software renderer llvmpipe will run on the CPU engine; that is what the x64 run did.

**Debug run:**

```bash
npm run tauri dev
```

The debug binary is `src-tauri/target/debug/loft`. GTK does not resize the notch unless a
window manager is running, so a bare Xvfb session is not enough for a normal desktop.

**Release:**

```bash
npm run tauri -- build --bundles deb,appimage
```

| File | Where |
|---|---|
| `Loft_<version>_amd64.deb` | `src-tauri/target/release/bundle/deb/` |
| `Loft_<version>_amd64.AppImage` | `src-tauri/target/release/bundle/appimage/` |

The release binary before bundling is `src-tauri/target/release/loft`. CPU and Vulkan from
`src-tauri/binaries/` are inside the package. There is no `scripts/linux/` release script
and no `releases/linux/` folder yet.

**Signing.** Unsigned. There is no Gatekeeper or SmartScreen step. The macOS and Windows
warnings, and what to do about them, are in
[First open](first-open.md).
