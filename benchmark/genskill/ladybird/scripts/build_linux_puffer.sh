#!/usr/bin/env bash
# Builds a Linux puffer binary for execution inside Ladybird Docker sandboxes.
#
# Usage: build_linux_puffer.sh

set -euo pipefail

ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
OUT="${PUFFER_LINUX_BIN:-$ROOT/benchmark/genskill/ladybird/.bin/puffer-linux}"
STAMP="$OUT.git-head"
CACHE_DIR="${PUFFER_LINUX_CACHE_DIR:-$ROOT/benchmark/genskill/ladybird/.cargo-linux}"
RUST_IMAGE="${PUFFER_LINUX_RUST_IMAGE:-rust:1-bookworm}"
FORCE="${FORCE_LINUX_PUFFER_BUILD:-0}"
HEAD=$(git -C "$ROOT" rev-parse HEAD 2>/dev/null || echo unknown)
CACHE_KEY="$HEAD:linux-build-v2"

mkdir -p "$(dirname "$OUT")"

if [ "$FORCE" != "1" ] && [ -x "$OUT" ]; then
  if [ -f "$STAMP" ] && [ "$(cat "$STAMP")" = "$CACHE_KEY" ]; then
    echo "Using existing Linux puffer binary at $OUT"
    exit 0
  fi
fi

mkdir -p "$CACHE_DIR/cargo-home" "$CACHE_DIR/target"

docker run --rm \
  -v "$ROOT:/work/puffer" \
  -e CARGO_HOME=/work/puffer/benchmark/genskill/ladybird/.cargo-linux/cargo-home \
  -e CARGO_TARGET_DIR=/work/puffer/benchmark/genskill/ladybird/.cargo-linux/target \
  --workdir /work/puffer \
  "$RUST_IMAGE" \
  bash -c 'set -euo pipefail; export PATH="/usr/local/cargo/bin:$PATH"; apt-get update; DEBIAN_FRONTEND=noninteractive apt-get install -y protobuf-compiler pkg-config libssl-dev libsqlite3-dev; cargo build -p puffer-cli --release; cp /work/puffer/benchmark/genskill/ladybird/.cargo-linux/target/release/puffer /work/puffer/benchmark/genskill/ladybird/.bin/puffer-linux'

chmod +x "$OUT"
echo "$CACHE_KEY" > "$STAMP"
echo "Built Linux puffer binary at $OUT"
