#!/usr/bin/env bash
# Runs the full Ladybird /genskill replay benchmark.
#
# Usage: run_all.sh [YYYY-MM-DD]
#
# This is intentionally guarded: it refuses to launch the 30 replay jobs until
# every corpus entry has expert_run.md plus direct/gepa SKILL.md files.

set -euo pipefail

ROOT="benchmark/genskill/ladybird"
RUN_DATE="${1:-$(date -u +%Y-%m-%d)}"

missing=0
for pr_dir in "$ROOT"/pr_corpus/pr-*/; do
  pr_dir="${pr_dir%/}"
  for required in \
    "expert_run.md" \
    "skills/direct/SKILL.md" \
    "skills/gepa/SKILL.md"
  do
    if [ ! -f "$pr_dir/$required" ]; then
      echo "missing: $pr_dir/$required" >&2
      missing=1
    fi
  done
done

if [ "$missing" -ne 0 ]; then
  echo "Refusing to run full eval until all expert runs and skills exist." >&2
  exit 1
fi

source ~/.cargo/env
cargo build -p puffer-cli --release
cargo build -p puffer-genskill-eval --release
cargo run --release -p puffer-genskill-eval -- validate

for pr_dir in "$ROOT"/pr_corpus/pr-*/; do
  pr=$(basename "${pr_dir%/}")
  for arm in no-skill direct gepa; do
    echo "=== $pr / $arm ==="
    cargo run --release -p puffer-genskill-eval -- replay "$pr" "$arm" \
      || echo "(replay failed; continuing)"
  done
done

cargo run --release -p puffer-genskill-eval -- aggregate "$RUN_DATE"
