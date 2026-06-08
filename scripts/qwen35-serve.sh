#!/usr/bin/env bash
# qwen35-serve.sh — run the local Qwen3.5-0.8B OpenAI-compatible server so the
# `qwen35` puffer provider (127.0.0.1:8088) works. Runtime: mlx-lm only.
#
# Usage: ./scripts/qwen35-serve.sh        # foreground
#        ./scripts/qwen35-serve.sh --bg    # background + healthcheck
# Env:   PUFFER_HOME (default ~/.puffer)

set -euo pipefail
PUFFER_HOME="${PUFFER_HOME:-$HOME/.puffer}"
SHIM="$PUFFER_HOME/bin/qwen35-shim.py"
PY="$PUFFER_HOME/venvs/qwen35/bin/python"
export QWEN35_MODEL="${QWEN35_MODEL:-$PUFFER_HOME/models/qwen3.5-0.8b}"

[ -x "$PY" ]  || { echo "runtime venv missing — run ./scripts/qwen35-install.sh first." >&2; exit 1; }
[ -f "$SHIM" ] || { echo "shim not installed — run ./scripts/qwen35-install.sh first." >&2; exit 1; }
[ -f "$QWEN35_MODEL/config.json" ] || { echo "model not found at $QWEN35_MODEL — run qwen35-install.sh." >&2; exit 1; }

if [ "${1:-}" = "--bg" ]; then
  LOG="$PUFFER_HOME/qwen35-serve.log"
  nohup "$PY" "$SHIM" > "$LOG" 2>&1 &
  echo "started (pid $!), log: $LOG"
  for _ in $(seq 1 90); do
    if curl -fsS -m2 http://127.0.0.1:8088/v1/models >/dev/null 2>&1; then echo "ready: http://127.0.0.1:8088/v1"; exit 0; fi
    sleep 1
  done
  echo "did not become ready; check $LOG" >&2; exit 1
else
  exec "$PY" "$SHIM"
fi
