#!/usr/bin/env bash
#
# Start (or restart) the Momo desktop app in dev mode.
#
# Idempotent: safe to run whether or not Momo is already running. It first
# stops any Momo instance from THIS repo and reaps the puffer daemon /
# subscriber children (which do NOT reliably exit when the app window closes,
# so they linger as orphans and keep holding e.g. a Telegram session), then
# launches a fresh `npm run tauri dev`. Run it again to restart.
#
# Why the cleanup matters: the Tauri WS (1431) and OAuth (1457) ports fail to
# bind *silently* if a previous instance still holds them — the window opens
# but never connects. Stopping the old instance first avoids that trap.
#
# Scope guard: only ever kills binaries under this repo's target/debug. It
# never touches a globally-installed ~/.cargo/bin/puffer.
#
# Usage:
#   apps/momo/scripts/dev.sh   # from anywhere
#   ./scripts/dev.sh           # from apps/momo
#
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
MOMO_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_ROOT="$(git -C "$MOMO_DIR" rev-parse --show-toplevel)"
DEBUG_DIR="$REPO_ROOT/target/debug"

echo "==> Stopping any running Momo / orphaned puffer daemon from this repo…"
# Match the absolute target/debug path so we never hit a global ~/.cargo/bin/puffer.
pkill -f "$DEBUG_DIR/momo"                    2>/dev/null || true
pkill -f "$DEBUG_DIR/puffer daemon"           2>/dev/null || true
pkill -f "$DEBUG_DIR/puffer __subscriber"     2>/dev/null || true
pkill -f "vite --host 127.0.0.1 --port 1466"  2>/dev/null || true
sleep 1

# Momo is puffer-only; chat needs `puffer` reachable. If it's not on PATH and
# the caller hasn't pointed MOMO_PUFFER_BIN somewhere, default to the in-tree
# build (tauri dev's beforeDevCommand compiles it if it's missing).
if [ -z "${MOMO_PUFFER_BIN:-}" ] && ! command -v puffer >/dev/null 2>&1; then
  export MOMO_PUFFER_BIN="$DEBUG_DIR/puffer"
  echo "==> 'puffer' not on PATH; pointing MOMO_PUFFER_BIN at the in-tree build:"
  echo "    $MOMO_PUFFER_BIN"
fi

echo "==> Starting Momo:  npm run tauri dev   (in $MOMO_DIR)"
echo "    Vite :1466  ·  backend WS :1431  ·  OAuth :1457"
echo "    Close the window or press Ctrl-C to stop."
cd "$MOMO_DIR"
exec npm run tauri dev
