#!/usr/bin/env bash
# Records an expert run for one PR.
#
# Usage: record_expert_run.sh pr-12345
#
# Spins up a ladybird sandbox container, applies the test files from
# the corpus on top of base_commit, then waits for the user to run
# puffer inside the container and solve the task. On exit the
# transcript is saved to expert_run.jsonl and converted to expert_run.md.
#
# Requires: docker, puffer binary on PATH, puffer-genskill-eval built.

set -euo pipefail

PR="${1:?usage: record_expert_run.sh <pr-id>}"
CORPUS_DIR="benchmark/genskill/ladybird/pr_corpus/$PR"
META="$CORPUS_DIR/meta.json"

[ -f "$META" ] || { echo "no meta.json at $META"; exit 1; }

BASE_SHA=$(jq -r '.base_commit' "$META")
TASK_PROMPT=$(jq -r '.task_prompt' "$META")
IMAGE_TAG="${IMAGE_TAG:-puffer-genskill-eval-ladybird}"

CONTAINER=$(docker run -d --rm \
  -v "$PWD:/host:ro" \
  -e BASE_SHA="$BASE_SHA" \
  --workdir /work/ladybird \
  "$IMAGE_TAG" \
  sleep infinity)

trap "docker rm -f $CONTAINER >/dev/null" EXIT

docker exec "$CONTAINER" git -C /work/ladybird reset --hard "$BASE_SHA"
docker exec "$CONTAINER" bash -c "cp -r /host/$CORPUS_DIR/tests/. /work/ladybird/"

echo "=== Expert run for $PR ==="
echo "Task: $TASK_PROMPT"
echo "Container: $CONTAINER"
echo
echo "Attach to the container and solve the task:"
echo "  docker exec -it $CONTAINER bash"
echo "Run puffer inside it, then exit. The session should write a"
echo "transcript to /tmp/puffer-session.jsonl inside the container."
echo
read -rp "Press enter once you've completed the run and exited puffer..."

docker cp "$CONTAINER:/tmp/puffer-session.jsonl" "$CORPUS_DIR/expert_run.jsonl"
echo "Saved transcript to $CORPUS_DIR/expert_run.jsonl"

cargo run -p puffer-genskill-eval -- transcript-to-md \
  --in "$CORPUS_DIR/expert_run.jsonl" \
  --out "$CORPUS_DIR/expert_run.md"
echo "Saved markdown to $CORPUS_DIR/expert_run.md"
