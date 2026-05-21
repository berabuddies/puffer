#!/usr/bin/env bash
# Creates synthetic expert transcripts and direct/gepa skills for all PRs.
#
# Usage: prepare_synthetic_eval_inputs.sh

set -euo pipefail

ROOT="benchmark/genskill/ladybird"

source ~/.cargo/env
cargo build -p puffer-cli --release
cargo build -p puffer-genskill-eval --release
bash "$ROOT/scripts/build_linux_puffer.sh"
cargo run --release -p puffer-genskill-eval -- validate

for pr_dir in "$ROOT"/pr_corpus/pr-*/; do
  pr=$(basename "${pr_dir%/}")
  echo "=== preparing synthetic eval inputs for $pr ==="
  bash "$ROOT/scripts/record_codex_expert_run.sh" "$pr"
  bash "$ROOT/scripts/generate_skills.sh" "$pr"
done
