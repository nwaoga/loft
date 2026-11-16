#!/usr/bin/env sh
# macOS / Linux counterpart of scripts/win-smoke.bat: runs one of the real-app
# tests in a debug build, in a throwaway profile.
#
#   scripts/smoke.sh                              the UI test (smoke-ui.js)
#   scripts/smoke.sh smoke-engines.js             engines, power, fallback, performance test
#   LOFT_SMOKE_KEEP=1 scripts/smoke.sh smoke-restart.js
#                                                 after the UI test: the chat history survived a restart
#   LOFT_ENGINES_DIR=/tmp/loft-smoke-engines scripts/smoke.sh smoke-optional-engines.js
#                                                 installs the optional engines (downloads them)
#
# The test's index, settings and chat history live in $LOFT_DATA_DIR (default
# /tmp/loft-smoke-profile) and its only folder is sample-docs: your own
# folders, index, settings and conversations are neither read nor changed. It
# does read the models you have downloaded, and uses the chat and embedding
# model you have selected, so download and select both in Loft first.
#
# Each step is printed as "[dev] ...". A run ends with "[dev] 17 DONE"
# (smoke-ui), "[dev] E-DONE", "[dev] R-DONE" or "[dev] I-DONE", or with
# "[dev] FAILED: ..." and the reason.
#
# NOTE: written alongside win-smoke.bat but never run by its author, who had
# only Windows. See HANDOVER.md.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
root=$(dirname "$here")
script=${1:-smoke-ui.js}
[ -f "$here/$script" ] || { echo "no such test script: $here/$script" >&2; exit 1; }

export LOFT_DATA_DIR="${LOFT_DATA_DIR:-/tmp/loft-smoke-profile}"
# A caller can point this at another folder (the Linux promo run uses a
# larger copy of sample-docs so indexing stays on screen).
export LOFT_SEED_FOLDER="${LOFT_SEED_FOLDER:-$root/sample-docs}"
export LOFT_DEV_EVAL="$here/$script"
mkdir -p "$LOFT_DATA_DIR"

# Where Loft keeps your own settings (Tauri's app-data folder for com.loft.desktop).
case "$(uname -s)" in
  Darwin) real="$HOME/Library/Application Support/com.loft.desktop" ;;
  *) real="${XDG_DATA_HOME:-$HOME/.local/share}/com.loft.desktop" ;;
esac

if [ "${LOFT_SMOKE_KEEP:-}" != "1" ]; then
  # Start from an empty profile that has only the model choice in it.
  for f in loft.db loft.db-wal loft.db-shm chats.db chats.db-wal chats.db-shm selection.json; do
    rm -f "$LOFT_DATA_DIR/$f"
  done
  if [ -f "$real/selection.json" ]; then
    node -e '
      const fs = require("fs");
      const own = JSON.parse(fs.readFileSync(process.argv[1], "utf8"));
      fs.writeFileSync(process.argv[2], JSON.stringify({ chat: own.chat ?? null, embed: own.embed ?? null }, null, 2));
    ' "$real/selection.json" "$LOFT_DATA_DIR/selection.json"
  else
    echo "no model selection found in $real: open Loft once, download and select a chat and an embedding model, then run this again" >&2
    exit 1
  fi
fi

cd "$root"
exec npm run tauri dev
