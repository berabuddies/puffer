#!/usr/bin/env bash
# build-cua-driver.sh — Build the vendored CUA computer-use driver and install
# it where the Puffer daemon's bundled `cua-computer` MCP server expects it.
#
# The driver (vendor/cua-driver, MIT) is its own cargo workspace, kept out of
# Puffer's build. This compiles it for the host platform and drops the binary at
# $PUFFER_HOME/bin/cua-driver (default ~/.puffer/bin/cua-driver), which the
# bundled MCP manifest (resources/mcp_servers/cua-computer.yaml) launches.
#
# Usage:  ./scripts/build-cua-driver.sh
# Env:    PUFFER_HOME (default ~/.puffer), CARGO (default: cargo on PATH)

set -euo pipefail
HERE="$(cd "$(dirname "$0")/.." && pwd)"
SRC="$HERE/vendor/cua-driver"
PUFFER_HOME="${PUFFER_HOME:-$HOME/.puffer}"
CARGO="${CARGO:-cargo}"
DEST="$PUFFER_HOME/bin"

[ -f "$SRC/Cargo.toml" ] || { echo "error: vendored driver not found at $SRC" >&2; exit 1; }
command -v "$CARGO" >/dev/null 2>&1 || { echo "error: cargo not found (set CARGO=)" >&2; exit 1; }

echo "building cua-driver (release) from $SRC …"
( cd "$SRC" && "$CARGO" build --release -p cua-driver )

mkdir -p "$DEST"
cp "$SRC/target/release/cua-driver" "$DEST/cua-driver"
chmod +x "$DEST/cua-driver"
echo "installed: $DEST/cua-driver ($("$DEST/cua-driver" --version 2>&1 | head -1))"

# Auto-register a user-scoped MCP server so the daemon (and thus the desktop
# app) exposes computer-use tools out of the box — no manual `puffer mcp add`.
# Written here (not committed) because the stdio target needs an absolute path
# and stdio targets are not env-expanded.
MANIFEST_DIR="$PUFFER_HOME/resources/mcp_servers"
mkdir -p "$MANIFEST_DIR"
cat > "$MANIFEST_DIR/cua-computer.yaml" <<YAML
id: cua-computer
display_name: Computer Use (CUA driver)
transport: stdio
target: $DEST/cua-driver mcp --no-daemon-relaunch
description: Drive this computer (screenshot/click/type/windows) via the vendored CUA driver.
YAML
echo "registered MCP server 'cua-computer' -> $MANIFEST_DIR/cua-computer.yaml"
echo
echo "Desktop users now get computer-use tools automatically. On macOS, grant"
echo "Screen Recording + Accessibility to the Puffer app/daemon on first use"
echo "(System Settings > Privacy & Security)."
