#!/usr/bin/env node
// Loft's version number lives in package.json; tauri.conf.json reads it from
// there ("version": "../package.json"), so the app, its About text and the
// installer all agree. This sets it, and keeps the Rust crate's copy
// (src-tauri/Cargo.toml) in step. Used by the release scripts; the same on
// every OS.
//
//   node scripts/set-version.mjs            print the current version
//   node scripts/set-version.mjs 1.2.0      set it
//   node scripts/set-version.mjs patch      0.1.0 -> 0.1.1 (also: minor, major)
//
// Versions are MAJOR.MINOR.PATCH, numbers only: the Windows MSI format
// accepts nothing else (and at most 255.255.65535).

import { readFileSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
const packagePath = join(root, "package.json");
const cargoPath = join(root, "src-tauri", "Cargo.toml");

const pkg = JSON.parse(readFileSync(packagePath, "utf8"));
const current = pkg.version;
const arg = process.argv[2];

if (!arg) {
  console.log(current);
  process.exit(0);
}

const parse = (v) => {
  const m = /^(\d+)\.(\d+)\.(\d+)$/.exec(v);
  if (!m) return null;
  const parts = m.slice(1).map(Number);
  if (parts[0] > 255 || parts[1] > 255 || parts[2] > 65535) return null;
  return parts;
};

let next;
if (["major", "minor", "patch"].includes(arg)) {
  const [major, minor, patch] = parse(current) ?? fail(`the current version "${current}" is not MAJOR.MINOR.PATCH`);
  next = arg === "major" ? `${major + 1}.0.0` : arg === "minor" ? `${major}.${minor + 1}.0` : `${major}.${minor}.${patch + 1}`;
} else {
  next = arg;
}
if (!parse(next)) fail(`"${next}" is not a version: use MAJOR.MINOR.PATCH, numbers only (e.g. 1.2.0), or major / minor / patch`);

// package.json: rewrite only the version line, so the file keeps its layout.
const pkgText = readFileSync(packagePath, "utf8");
const newPkg = pkgText.replace(/("version"\s*:\s*")[^"]*(")/, `$1${next}$2`);
if (newPkg === pkgText && current !== next) fail("could not find the version in package.json");
writeFileSync(packagePath, newPkg);

// Cargo.toml: the first `version = "..."`, which is the [package] one.
const cargoText = readFileSync(cargoPath, "utf8");
const packageSection = cargoText.indexOf("[package]");
const match = packageSection < 0 ? null : /^version\s*=\s*"[^"]*"/m.exec(cargoText.slice(packageSection));
if (!match) fail("could not find the [package] version in src-tauri/Cargo.toml");
const at = packageSection + match.index;
writeFileSync(cargoPath, cargoText.slice(0, at) + `version = "${next}"` + cargoText.slice(at + match[0].length));

console.log(next);

function fail(message) {
  console.error(`set-version: ${message}`);
  process.exit(1);
}
