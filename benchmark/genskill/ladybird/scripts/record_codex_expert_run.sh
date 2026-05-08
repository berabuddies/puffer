#!/usr/bin/env bash
# Records a synthetic expert run for one PR using puffer non-interactive.
#
# Usage: record_codex_expert_run.sh pr-12345
#
# This is intentionally named "codex expert": it produces model-generated
# expert_run.{jsonl,md} files for pipeline validation and synthetic-expert
# benchmark runs, not human expert traces.

set -euo pipefail

PR="${1:?usage: record_codex_expert_run.sh <pr-id>}"
ROOT="benchmark/genskill/ladybird"
CORPUS_DIR="$ROOT/pr_corpus/$PR"
META="$CORPUS_DIR/meta.json"
IMAGE_TAG="${IMAGE_TAG:-puffer-genskill-eval-ladybird}"
HOST_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
PUFFER_LINUX_BIN="${PUFFER_LINUX_BIN:-$HOST_ROOT/benchmark/genskill/ladybird/.bin/puffer-linux}"
PROVIDER="${PUFFER_EVAL_PROVIDER:-openai}"
MODEL="${PUFFER_EVAL_MODEL:-${PUFFER_MODEL:-gpt-5.4}}"
EFFORT="${PUFFER_EVAL_EFFORT:-${PUFFER_EFFORT:-}}"
MAX_TOOL_CALLS="${EXPERT_TOOL_BUDGET:-80}"
MAX_TOKENS="${EXPERT_TOKEN_BUDGET:-400000}"
FORCE="${FORCE:-0}"

[ -f "$META" ] || { echo "no meta.json at $META" >&2; exit 1; }
[ -x "$PUFFER_LINUX_BIN" ] || {
  echo "missing Linux puffer binary at $PUFFER_LINUX_BIN" >&2
  echo "run: bash benchmark/genskill/ladybird/scripts/build_linux_puffer.sh" >&2
  exit 1
}

if [ "$FORCE" != "1" ] && [ -f "$CORPUS_DIR/expert_run.md" ] && [ -f "$CORPUS_DIR/expert_run.jsonl" ]; then
  echo "Synthetic expert run already exists for $PR; set FORCE=1 to regenerate."
  exit 0
fi

PUFFER_LINUX_BIN_ABS="$(cd "$(dirname "$PUFFER_LINUX_BIN")" && pwd -P)/$(basename "$PUFFER_LINUX_BIN")"
BASE_SHA=$(jq -r '.base_commit' "$META")
TASK_PROMPT=$(jq -r '.task_prompt' "$META")

docker_args=(
  run -d --rm
  -v "$PUFFER_LINUX_BIN_ABS:/usr/local/bin/puffer:ro"
  -v "$HOST_ROOT:/host:ro"
  -e HOME=/home/ladybird
  -e OPENAI_API_KEY
  -e ANTHROPIC_API_KEY
  -e OPENAI_BASE_URL
  -e OPENAI_ORGANIZATION
  -e OPENAI_PROJECT
  -e PUFFER_PROVIDER
  -e PUFFER_MODEL
  -e PUFFER_EFFORT
  -e PUFFER_EVAL_PROVIDER
  -e PUFFER_EVAL_MODEL
  -e PUFFER_EVAL_EFFORT
)

if [ -d "$HOME/.codex" ]; then
  docker_args+=(-v "$HOME/.codex:/home/ladybird/.codex:ro")
fi

docker_args+=(
  --user ladybird
  --workdir /work/ladybird
  "$IMAGE_TAG"
  sleep infinity
)

CONTAINER=$(docker "${docker_args[@]}")
trap 'docker rm -f "$CONTAINER" >/dev/null 2>&1 || true' EXIT

docker exec --user ladybird "$CONTAINER" git -C /work/ladybird reset --hard "$BASE_SHA"
docker exec --user ladybird "$CONTAINER" bash -lc "cp -r /host/$CORPUS_DIR/tests/. /work/ladybird/"

puffer_args=(
  puffer non-interactive
  --provider "$PROVIDER"
  --model "$MODEL"
  --user-message "$TASK_PROMPT"
  --transcript-out /tmp/puffer-session.jsonl
  --emit-artifact /tmp/expert-artifact.json
  --artifact-pr "$PR"
  --artifact-arm no-skill
  --max-tool-calls "$MAX_TOOL_CALLS"
  --max-tokens "$MAX_TOKENS"
)

if [ -n "$EFFORT" ]; then
  puffer_args+=(--effort "$EFFORT")
fi

echo "=== Synthetic Codex expert run for $PR ==="
echo "Provider/model: $PROVIDER / $MODEL"
set +e
docker exec --user ladybird "$CONTAINER" "${puffer_args[@]}"
status=$?
set -e

if ! docker exec --user ladybird "$CONTAINER" test -s /tmp/puffer-session.jsonl; then
  echo "puffer did not produce /tmp/puffer-session.jsonl" >&2
  exit "$status"
fi

docker cp "$CONTAINER:/tmp/puffer-session.jsonl" "$CORPUS_DIR/expert_run.jsonl"
if docker exec --user ladybird "$CONTAINER" test -s /tmp/expert-artifact.json; then
  docker cp "$CONTAINER:/tmp/expert-artifact.json" "$CORPUS_DIR/expert_run_artifact.json"
fi

cargo run -p puffer-genskill-eval -- transcript-to-md \
  --in "$CORPUS_DIR/expert_run.jsonl" \
  --out "$CORPUS_DIR/expert_run.md"

echo "Saved $CORPUS_DIR/expert_run.{jsonl,md}"
exit "$status"
