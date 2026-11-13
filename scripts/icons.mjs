#!/usr/bin/env node
// Makes every app icon in src-tauri/icons from app-icon.svg (`npm run icons`).
// The icons are checked in, so this is only needed after changing the
// design. Tauri's generator also makes Android and iOS sets, which a desktop
// app does not use; those are removed.

import { execSync } from "node:child_process";
import { rmSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

const root = join(dirname(fileURLToPath(import.meta.url)), "..");
execSync("npx tauri icon app-icon.svg", { cwd: root, stdio: "inherit" });
for (const mobile of ["android", "ios"]) {
  rmSync(join(root, "src-tauri", "icons", mobile), { recursive: true, force: true });
}
