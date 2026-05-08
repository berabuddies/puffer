#!/usr/bin/env bash
# Builds a Linux puffer binary for execution inside Ladybird Docker sandboxes.
#
# Usage: build_linux_puffer.sh

set -euo pipefail

ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
OUT="${PUFFER_LINUX_BIN:-$ROOT/benchmark/genskill/ladybird/.bin/puffer-linux}"
RUST_IMAGE="${PUFFER_LINUX_RUST_IMAGE:-rust:1-bookworm}"

mkdir -p "$(dirname "$OUT")"

docker run --rm \
  -v "$ROOT:/work/puffer" \
  -e CARGO_HOME=/tmp/cargo-home \
  -e CARGO_TARGET_DIR=/tmp/puffer-target \
  --workdir /work/puffer \
  "$RUST_IMAGE" \
  bash -c 'set -euo pipefail; export PATH="/usr/local/cargo/bin:$PATH"; apt-get update; DEBIAN_FRONTEND=noninteractive apt-get install -y protobuf-compiler pkg-config libssl-dev libsqlite3-dev; cargo build -p puffer-cli --release; cp /tmp/puffer-target/release/puffer /work/puffer/benchmark/genskill/ladybird/.bin/puffer-linux'

chmod +x "$OUT"
echo "Built Linux puffer binary at $OUT"
