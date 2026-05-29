#!/usr/bin/env bash
# minicpm5-serve.sh — run the local MiniCPM5-1B OpenAI-compatible server so the
# `minicpm5` puffer provider (127.0.0.1:8088) works. Runtime: mlx-lm only.
#
# Usage: ./scripts/minicpm5-serve.sh        # foreground
#        ./scripts/minicpm5-serve.sh --bg    # background + healthcheck
# Env:   PUFFER_HOME (default ~/.puffer)

set -euo pipefail
PUFFER_HOME="${PUFFER_HOME:-$HOME/.puffer}"
SHIM="$PUFFER_HOME/bin/minicpm5-shim.py"
PY="$PUFFER_HOME/venvs/minicpm5/bin/python"
export MINICPM5_MODEL="${MINICPM5_MODEL:-$PUFFER_HOME/models/minicpm5-1b}"

[ -x "$PY" ]  || { echo "runtime venv missing — run ./scripts/minicpm5-install.sh first." >&2; exit 1; }
[ -f "$SHIM" ] || { echo "shim not installed — run ./scripts/minicpm5-install.sh first." >&2; exit 1; }
[ -f "$MINICPM5_MODEL/config.json" ] || { echo "model not found at $MINICPM5_MODEL — run minicpm5-install.sh." >&2; exit 1; }

if [ "${1:-}" = "--bg" ]; then
  LOG="$PUFFER_HOME/minicpm5-serve.log"
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
