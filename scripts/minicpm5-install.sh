#!/usr/bin/env bash
# minicpm5-install.sh — install the local MiniCPM5-1B model for puffer (macOS).
#
# Runtime is just the mlx-lm pip package + the model weights — no Docker, no
# separate server binary (mlx runs natively on Apple Silicon). This guides the
# download: ensures mlx-lm, fetches the 4-bit MLX weights, installs the shim,
# and registers the `minicpm5` provider. Then `minicpm5-serve.sh` runs it.
#
# Usage: ./scripts/minicpm5-install.sh
# Env:   PUFFER_HOME (default ~/.puffer), MINICPM5_REPO (default openbmb/MiniCPM5-1B-MLX)

set -euo pipefail

if [ "$(uname -s)" != "Darwin" ]; then
  echo "MiniCPM5 local install is for macOS (Apple Silicon mlx). Detected: $(uname -s). Skipping." >&2
  exit 2
fi
if [ "$(uname -m)" != "arm64" ]; then
  echo "warning: not arm64 — mlx is optimized for Apple Silicon; continuing anyway." >&2
fi

PUFFER_HOME="${PUFFER_HOME:-$HOME/.puffer}"
REPO="${MINICPM5_REPO:-openbmb/MiniCPM5-1B-MLX}"
MODEL_DIR="$PUFFER_HOME/models/minicpm5-1b"
BIN_DIR="$PUFFER_HOME/bin"
HERE="$(cd "$(dirname "$0")/.." && pwd)"

# Cross-process single-flight: two concurrent installers (e.g. a second window)
# would interleave snapshot_download / pip into the same dirs and corrupt them.
# mkdir is atomic; flock isn't available on stock macOS.
mkdir -p "$PUFFER_HOME"
LOCK="$PUFFER_HOME/.minicpm5-install.lock"
if ! mkdir "$LOCK" 2>/dev/null; then
  echo "error: another MiniCPM5 install is already running (lock: $LOCK)." >&2
  echo "       if you're sure none is, remove it: rmdir '$LOCK'" >&2
  exit 1
fi
trap 'rmdir "$LOCK" 2>/dev/null || true' EXIT

command -v python3 >/dev/null 2>&1 || { echo "error: python3 not found (macOS ships it; or 'brew install python')." >&2; exit 1; }

# Isolated runtime venv — avoids PEP 668 (homebrew/system python) and keeps the
# model deps off the user's global python. Runtime is just mlx-lm + hf_hub.
VENV="$PUFFER_HOME/venvs/minicpm5"
PY="$VENV/bin/python"
echo "1/4 ensuring runtime venv (mlx-lm) → $VENV …"
if [ ! -x "$PY" ]; then python3 -m venv "$VENV"; fi
if ! "$PY" -c "import mlx_lm" >/dev/null 2>&1; then
  "$PY" -m pip install --quiet --upgrade pip
  "$PY" -m pip install --quiet --upgrade mlx-lm huggingface_hub
fi
echo "    runtime: $("$PY" -c 'import mlx_lm;print("mlx-lm",mlx_lm.__version__)')"

echo "2/4 downloading model weights ($REPO, ~0.6GB) → $MODEL_DIR …"
if [ -f "$MODEL_DIR/config.json" ]; then
  echo "    already present, skipping download."
else
  mkdir -p "$MODEL_DIR"
  # Pass repo/dir via env (argv), not string-interpolated into Python source —
  # a path/repo containing quotes must not break or inject code.
  MC5_REPO="$REPO" MC5_DIR="$MODEL_DIR" "$PY" - <<'PY'
import os
from huggingface_hub import snapshot_download
snapshot_download(os.environ["MC5_REPO"], local_dir=os.environ["MC5_DIR"])
PY
fi

echo "3/4 installing shim → $BIN_DIR/minicpm5-shim.py …"
mkdir -p "$BIN_DIR"
# Prefer the in-repo shim if vendored; else the local eval copy.
SHIM_SRC="$HERE/scripts/minicpm5_shim.py"
[ -f "$SHIM_SRC" ] || SHIM_SRC="$HOME/ai/minicpm5-1b-eval/minicpm_shim.py"
cp "$SHIM_SRC" "$BIN_DIR/minicpm5-shim.py"

echo "4/4 registering provider → $PUFFER_HOME/resources/providers/minicpm5.yaml …"
mkdir -p "$PUFFER_HOME/resources/providers"
# Don't swallow failure — registration is the whole point; surface it.
cp "$HERE/resources/providers/minicpm5.yaml" "$PUFFER_HOME/resources/providers/minicpm5.yaml"

echo
echo "Done. MiniCPM5-1B installed (model: $MODEL_DIR)."
echo "Start it:  ./scripts/minicpm5-serve.sh"
echo "Then in puffer it's the 'minicpm5' provider (model minicpm5-1b) — no API key."
