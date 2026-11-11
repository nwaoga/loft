#!/usr/bin/env node
// Fetch the prebuilt llama.cpp server for this machine into
//   src-tauri/binaries/<platform>-<accel>/
// which is where Loft looks for it (dev) and what gets bundled (release).
//
//   node scripts/fetch-llama.mjs            # this platform's default builds
//   node scripts/fetch-llama.mjs cpu        # only the named builds
//   node scripts/fetch-llama.mjs cpu vulkan
//
// Every archive is checked against the SHA-256 GitHub publishes for it.
// Only `llama-server` and the libraries it loads are kept.
// Set LLAMA_TAG to use a different llama.cpp release.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, chmodSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

const TAG = process.env.LLAMA_TAG ?? "b12110";
const REPO = "ggml-org/llama.cpp";

// platform -> accel -> release asset (without the "llama-<tag>-" prefix).
// The first entries are the defaults. CUDA is not fetched here: it needs two
// archives per CUDA version; unpack them into <platform>-cuda/ by hand.
const BUILDS = {
  "win-x64": { cpu: "bin-win-cpu-x64.zip", vulkan: "bin-win-vulkan-x64.zip" },
  "win-arm64": { cpu: "bin-win-cpu-arm64.zip", vulkan: "bin-win-vulkan-arm64.zip" },
  "mac-arm64": { metal: "bin-macos-arm64.tar.gz" },
  "mac-x64": { cpu: "bin-macos-x64.tar.gz" },
  "linux-x64": { cpu: "bin-ubuntu-x64.tar.gz", vulkan: "bin-ubuntu-vulkan-x64.tar.gz" },
  "linux-arm64": { cpu: "bin-ubuntu-arm64.tar.gz", vulkan: "bin-ubuntu-vulkan-arm64.tar.gz" },
};

function platformKey() {
  const os = { win32: "win", darwin: "mac", linux: "linux" }[process.platform];
  const arch = { x64: "x64", arm64: "arm64" }[process.arch];
  if (!os || !arch) throw new Error(`unsupported platform: ${process.platform}/${process.arch}`);
  return `${os}-${arch}`;
}

const isWindows = process.platform === "win32";
const serverName = isWindows ? "llama-server.exe" : "llama-server";

/** Keep the server, the shared libraries it loads, and the licences. */
function keep(name) {
  if (name === serverName || /^LICENSE/i.test(name)) return true;
  const isLibrary = /\.(dll|dylib)$/.test(name) || /\.so(\.\d+)*$/.test(name);
  if (!isLibrary) return false; // every other file is another tool's executable
  // "<tool>-impl" libraries belong to the other command-line tools.
  return !/-impl\./.test(name) || name.includes("llama-server-impl");
}

function run(cmd, args) {
  const r = spawnSync(cmd, args, { stdio: "inherit" });
  if (r.error) throw r.error;
  if (r.status !== 0) throw new Error(`${cmd} exited with ${r.status}`);
}

function extract(archive, into) {
  if (isWindows) {
    // The bsdtar that ships with Windows 10+ reads both .zip and .tar.gz.
    // (A `tar` found on PATH may be GNU tar from Git, which cannot unzip.)
    run(join(process.env.SystemRoot ?? "C:\\Windows", "System32", "tar.exe"), ["-xf", archive, "-C", into]);
  } else if (archive.endsWith(".zip")) {
    run("unzip", ["-q", archive, "-d", into]);
  } else {
    run("tar", ["-xf", archive, "-C", into]);
  }
}

function findServerDir(dir) {
  for (const entry of readdirSync(dir, { withFileTypes: true })) {
    const full = join(dir, entry.name);
    if (entry.isDirectory()) {
      const found = findServerDir(full);
      if (found) return found;
    } else if (entry.name === serverName) {
      return dir;
    }
  }
  return null;
}

async function download(asset, dest) {
  const res = await fetch(asset.browser_download_url, { redirect: "follow" });
  if (!res.ok) throw new Error(`download failed: HTTP ${res.status} for ${asset.name}`);
  const hash = createHash("sha256");
  const body = Readable.fromWeb(res.body);
  body.on("data", (chunk) => hash.update(chunk));
  await pipeline(body, createWriteStream(dest));
  const actual = hash.digest("hex");
  const expected = (asset.digest ?? "").replace(/^sha256:/, "");
  if (!expected) throw new Error(`GitHub published no checksum for ${asset.name}; refusing to use it`);
  if (actual !== expected) {
    rmSync(dest, { force: true });
    throw new Error(`checksum mismatch for ${asset.name}\n  expected ${expected}\n  got      ${actual}`);
  }
}

async function main() {
  const platform = platformKey();
  const builds = BUILDS[platform];
  const wanted = process.argv.slice(2);
  const accels = wanted.length ? wanted : Object.keys(builds);
  for (const accel of accels) {
    if (!builds[accel]) {
      throw new Error(`no "${accel}" build for ${platform}; available: ${Object.keys(builds).join(", ")}`);
    }
  }

  const res = await fetch(`https://api.github.com/repos/${REPO}/releases/tags/${TAG}`, {
    headers: { "User-Agent": "fetch-llama", Accept: "application/vnd.github+json" },
  });
  if (!res.ok) throw new Error(`could not read release ${TAG}: HTTP ${res.status}`);
  const release = await res.json();

  const root = join(dirname(fileURLToPath(import.meta.url)), "..", "src-tauri", "binaries");
  const work = mkdtempSync(join(tmpdir(), "loft-llama-"));
  try {
    for (const accel of accels) {
      const name = `llama-${TAG}-${builds[accel]}`;
      const asset = release.assets.find((a) => a.name === name);
      if (!asset) throw new Error(`release ${TAG} has no asset named ${name}`);

      const archive = join(work, name);
      process.stdout.write(`${platform}-${accel}: downloading ${name} (${(asset.size / 1e6).toFixed(1)} MB)… `);
      await download(asset, archive);
      console.log("checksum ok");

      const unpacked = join(work, `${accel}-unpacked`);
      mkdirSync(unpacked);
      extract(archive, unpacked);
      const from = findServerDir(unpacked);
      if (!from) throw new Error(`${name} does not contain ${serverName}`);

      const dest = join(root, `${platform}-${accel}`);
      rmSync(dest, { recursive: true, force: true });
      mkdirSync(dest, { recursive: true });
      let kept = 0;
      let bytes = 0;
      for (const file of readdirSync(from)) {
        if (!keep(file)) continue;
        const src = join(from, file);
        if (!statSync(src).isFile()) continue; // follows symlinks; skips dirs
        // Copy symlinked library names as real files so the folder can be
        // bundled as-is on every OS.
        cpSync(src, join(dest, file), { dereference: true });
        kept += 1;
        bytes += statSync(join(dest, file)).size;
      }
      if (!isWindows) chmodSync(join(dest, serverName), 0o755);
      if (!existsSync(join(dest, serverName))) throw new Error(`failed to install ${serverName}`);
      console.log(`${platform}-${accel}: ${kept} files, ${(bytes / 1e6).toFixed(1)} MB -> ${dest}`);
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(`fetch-llama: ${err.message}`);
  process.exit(1);
});
