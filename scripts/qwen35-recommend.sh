#!/usr/bin/env bash
# qwen35-recommend.sh — should puffer recommend installing the local Qwen3.5
# model to this user? Emits a JSON decision the desktop onboarding can render as
# a "You're on macOS — install a local model?" card. Pure detection, no install.
#
#   recommend=true  → macOS + Apple Silicon + not yet installed
#   recommend=false → wrong OS/arch, or already installed (with a reason)
#
# Usage: ./scripts/qwen35-recommend.sh        # prints JSON, exit 0 if recommend
# Env:   PUFFER_HOME (default ~/.puffer)

PUFFER_HOME="${PUFFER_HOME:-$HOME/.puffer}"
MODEL="$PUFFER_HOME/models/qwen3.5-0.8b/config.json"

emit() { # recommend reason
  printf '{"recommend":%s,"reason":"%s","model":"qwen3.5-0.8b","display_name":"Qwen3.5-0.8B (local)","why":"on-device user-behavior analysis — private, free, always-on","size":"~992MB","install_cmd":"scripts/qwen35-install.sh"}\n' "$1" "$2"
}

os="$(uname -s)"; arch="$(uname -m)"
if [ "$os" != "Darwin" ]; then
  emit false "not macOS ($os)"; exit 1
fi
if [ "$arch" != "arm64" ]; then
  emit false "not Apple Silicon ($arch) — mlx is optimized for arm64"; exit 1
fi
if [ -f "$MODEL" ]; then
  emit false "already installed"; exit 1
fi
emit true "macOS Apple Silicon, model not yet installed"
exit 0
