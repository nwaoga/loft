#!/usr/bin/env node
// Fetch prebuilt llama.cpp servers ("engines") for this machine into
//   src-tauri/binaries/<platform>-<backend>/
// which is where Loft looks for them (dev) and what gets bundled (release).
//
//   node scripts/fetch-llama.mjs                 # this platform's standard engines (CPU, Vulkan / Metal)
//   node scripts/fetch-llama.mjs cuda            # an optional engine: NVIDIA CUDA (about 600 MB)
//   node scripts/fetch-llama.mjs openvino        # an optional engine: Intel OpenVINO, incl. NPU (experimental)
//   node scripts/fetch-llama.mjs cpu vulkan cuda # any combination
//   node scripts/fetch-llama.mjs --list          # what exists for this platform
//
// The builds, their sizes and their SHA-256 checksums are pinned in
// src-tauri/engines.json (one llama.cpp release, so every engine is the same
// revision). An archive that does not match its pinned checksum is discarded.
// Only `llama-server`, the libraries and data files it loads, and the
// licences are kept. An optional engine can also be installed from inside
// Loft (Settings > Performance & Battery), which puts it in the app-data
// folder instead; anything fetched here is bundled into the installer.
//
// To move to another llama.cpp release: edit TAG in
// scripts/make-engines-manifest.py and run it to re-pin the checksums.

import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { cpSync, chmodSync, createWriteStream, existsSync, mkdirSync, mkdtempSync, readFileSync, readdirSync, rmSync, statSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { Readable } from "node:stream";
import { pipeline } from "node:stream/promises";
import { fileURLToPath } from "node:url";

const here = dirname(fileURLToPath(import.meta.url));
const manifest = JSON.parse(readFileSync(join(here, "..", "src-tauri", "engines.json"), "utf8"));

function platformKey() {
  const os = { win32: "win", darwin: "mac", linux: "linux" }[process.platform];
  const arch = { x64: "x64", arm64: "arm64" }[process.arch];
  if (!os || !arch) throw new Error(`unsupported platform: ${process.platform}/${process.arch}`);
  return `${os}-${arch}`;
}

const isWindows = process.platform === "win32";
const serverName = isWindows ? "llama-server.exe" : "llama-server";

/** Keep the server, what it loads, and the licences; drop the other tools. */
function keep(name) {
  if (name === serverName || /^LICENSE/i.test(name)) return true;
  const isLibrary = /\.(dll|dylib)$/.test(name) || /\.so(\.\d+)*$/.test(name);
  // Data files a backend's runtime reads at start-up (OpenVINO plugins).
  const isData = /\.(xml|json|bin|blob|mvcmd|elf)$/.test(name);
  if (!isLibrary && !isData) return false; // every other file is another tool's executable
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

/** The directory that holds `llama-server`, or the one holding the most libraries. */
function payloadDir(dir) {
  let best = null;
  const visit = (d) => {
    const entries = readdirSync(d, { withFileTypes: true });
    if (entries.some((e) => e.isFile() && e.name === serverName)) return d;
    const libraries = entries.filter((e) => !e.isDirectory() && keep(e.name)).length;
    if (libraries > (best?.libraries ?? 0)) best = { dir: d, libraries };
    for (const e of entries) {
      if (!e.isDirectory()) continue;
      const found = visit(join(d, e.name));
      if (found) return found;
    }
    return null;
  };
  return visit(dir) ?? best?.dir ?? null;
}

async function download(archive, dest) {
  const res = await fetch(archive.url, { redirect: "follow" });
  if (!res.ok) throw new Error(`download failed: HTTP ${res.status} for ${archive.name}`);
  const hash = createHash("sha256");
  const body = Readable.fromWeb(res.body);
  body.on("data", (chunk) => hash.update(chunk));
  await pipeline(body, createWriteStream(dest));
  const actual = hash.digest("hex");
  if (actual !== archive.sha256) {
    rmSync(dest, { force: true });
    throw new Error(`checksum mismatch for ${archive.name}\n  pinned ${archive.sha256}\n  got    ${actual}`);
  }
}

const megabytes = (bytes) => `${(bytes / 1e6).toFixed(1)} MB`;

async function main() {
  const platform = platformKey();
  const builds = manifest.builds[platform];
  if (!builds) throw new Error(`no llama.cpp builds are pinned for ${platform}`);
  const args = process.argv.slice(2);

  if (args.includes("--list")) {
    console.log(`llama.cpp ${manifest.tag} for ${platform}:`);
    for (const [backend, build] of Object.entries(builds)) {
      const size = megabytes(build.archives.reduce((n, a) => n + a.size, 0));
      const installed = existsSync(join(here, "..", "src-tauri", "binaries", `${platform}-${backend}`, serverName));
      console.log(`  ${backend.padEnd(9)} ${build.optional ? "optional" : "standard"}  ${size.padStart(9)}  ${installed ? "installed" : "not installed"}${build.note ? `  — ${build.note}` : ""}`);
    }
    return;
  }

  const wanted = args.length ? args : Object.keys(builds).filter((b) => !builds[b].optional);
  for (const backend of wanted) {
    if (!builds[backend]) {
      throw new Error(`no "${backend}" build for ${platform}; available: ${Object.keys(builds).join(", ")}`);
    }
  }

  const root = join(here, "..", "src-tauri", "binaries");
  const work = mkdtempSync(join(tmpdir(), "loft-llama-"));
  try {
    for (const backend of wanted) {
      const build = builds[backend];
      const dest = join(root, `${platform}-${backend}`);
      const staging = join(work, `${backend}-staging`);
      mkdirSync(staging);
      let kept = 0;
      let bytes = 0;
      // An engine can come in several archives (CUDA: the build, and NVIDIA's
      // runtime libraries); they are merged into one folder.
      for (const [i, archive] of build.archives.entries()) {
        const file = join(work, archive.name);
        process.stdout.write(`${platform}-${backend}: downloading ${archive.name} (${megabytes(archive.size)})… `);
        await download(archive, file);
        console.log("checksum ok");

        const unpacked = join(work, `${backend}-unpacked-${i}`);
        mkdirSync(unpacked);
        extract(file, unpacked);
        rmSync(file, { force: true });
        const from = payloadDir(unpacked);
        if (!from) throw new Error(`${archive.name} contains nothing Loft uses`);
        for (const name of readdirSync(from)) {
          if (!keep(name)) continue;
          const src = join(from, name);
          if (!statSync(src).isFile()) continue; // follows symlinks; skips dirs
          // Copy symlinked library names as real files so the folder can be
          // bundled as-is on every OS.
          cpSync(src, join(staging, name), { dereference: true });
          kept += 1;
          bytes += statSync(join(staging, name)).size;
        }
        rmSync(unpacked, { recursive: true, force: true });
      }
      if (!existsSync(join(staging, serverName))) throw new Error(`the ${backend} build does not contain ${serverName}`);
      if (!isWindows) chmodSync(join(staging, serverName), 0o755);
      // Replace the old engine only once the new one is complete.
      rmSync(dest, { recursive: true, force: true });
      mkdirSync(root, { recursive: true });
      cpSync(staging, dest, { recursive: true });
      console.log(`${platform}-${backend}: ${kept} files, ${megabytes(bytes)} -> ${dest}`);
    }
  } finally {
    rmSync(work, { recursive: true, force: true });
  }
}

main().catch((err) => {
  console.error(`fetch-llama: ${err.message}`);
  process.exit(1);
});
