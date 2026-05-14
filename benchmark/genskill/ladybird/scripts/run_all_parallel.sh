#!/usr/bin/env bash
# Runs the full Ladybird /genskill replay benchmark with bounded parallelism.
#
# Usage: JOBS=4 run_all_parallel.sh [RUN_DATE]

set -euo pipefail

ROOT="benchmark/genskill/ladybird"
RUN_DATE="${1:-$(date -u +%Y-%m-%d)}"
REPORT_DIR="$ROOT/reports/$RUN_DATE"
JOBS="${JOBS:-4}"
export RUN_DATE

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

mkdir -p "$REPORT_DIR"
rm -f "$REPORT_DIR"/pr-*.json "$REPORT_DIR"/summary.md

source ~/.cargo/env
cargo build -p puffer-cli --release
cargo build -p puffer-genskill-eval --release
bash "$ROOT/scripts/build_linux_puffer.sh"
export PUFFER_REPLAY_BIN="${PUFFER_REPLAY_BIN:-$ROOT/.bin/puffer-linux}"
cargo run --release -p puffer-genskill-eval -- validate

task_file=$(mktemp "${TMPDIR:-/tmp}/puffer-genskill-prs.XXXXXX")
trap 'rm -f "$task_file"' EXIT

for pr_dir in "$ROOT"/pr_corpus/pr-*/; do
  pr=$(basename "${pr_dir%/}")
  printf '%s\n' "$pr" >> "$task_file"
done

pr_count=$(wc -l < "$task_file" | tr -d ' ')
echo "Running $((pr_count * 3)) replays across $pr_count PRs with JOBS=$JOBS into $REPORT_DIR"
echo "Each PR runs no-skill, direct, and gepa serially to avoid triple-building the same Ladybird target."

xargs -P "$JOBS" -n 1 sh -c '
  pr="$1"
  for arm in no-skill direct gepa; do
    echo "=== $pr / $arm ==="
    cargo run --release -p puffer-genskill-eval -- replay "$pr" "$arm" --run-date "$RUN_DATE" \
      || echo "(replay failed; continuing): $pr / $arm" >&2
  done
' sh < "$task_file"

cargo run --release -p puffer-genskill-eval -- aggregate "$RUN_DATE"
