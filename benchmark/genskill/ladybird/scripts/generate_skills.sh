#!/usr/bin/env bash
# Generates direct and GEPA skills for one PR from an expert transcript.
#
# Usage: generate_skills.sh pr-12345

set -euo pipefail

PR="${1:?usage: generate_skills.sh <pr-id>}"
ROOT="benchmark/genskill/ladybird"
CORPUS_DIR="$ROOT/pr_corpus/$PR"
EXPERT_MD="$CORPUS_DIR/expert_run.md"
EXPERT_JSONL="$CORPUS_DIR/expert_run.jsonl"
PUFFER_BIN="${PUFFER_BIN:-target/release/puffer}"
PROVIDER="${PUFFER_EVAL_PROVIDER:-openai}"
MODEL="${PUFFER_EVAL_MODEL:-${PUFFER_MODEL:-gpt-5.4}}"
EFFORT="${PUFFER_EVAL_EFFORT:-${PUFFER_EFFORT:-}}"
DIRECT_TOOL_CALLS="${DIRECT_SKILL_TOOL_BUDGET:-20}"
DIRECT_TOKENS="${DIRECT_SKILL_TOKEN_BUDGET:-120000}"
GENSKILL_TOOL_CALLS="${GENSKILL_TOOL_BUDGET:-80}"
GENSKILL_TOKENS="${GENSKILL_TOKEN_BUDGET:-300000}"
GENSKILL_CANDIDATES="${GENSKILL_CANDIDATES:-3}"
GENSKILL_ROUNDS="${GENSKILL_ROUNDS:-2}"
FORCE="${FORCE:-0}"

[ -f "$EXPERT_MD" ] || { echo "no expert_run.md at $EXPERT_MD" >&2; exit 1; }
[ -f "$EXPERT_JSONL" ] || { echo "no expert_run.jsonl at $EXPERT_JSONL" >&2; exit 1; }
[ -x "$PUFFER_BIN" ] || { echo "missing executable puffer at $PUFFER_BIN" >&2; exit 1; }

mkdir -p "$CORPUS_DIR/skills/direct" "$CORPUS_DIR/skills/gepa"

HOST_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
PUFFER_BIN_ABS="$(cd "$(dirname "$PUFFER_BIN")" && pwd -P)/$(basename "$PUFFER_BIN")"
EXPERT_MD_ABS="$HOST_ROOT/$EXPERT_MD"
EXPERT_JSONL_ABS="$HOST_ROOT/$EXPERT_JSONL"
DIRECT_OUT="$HOST_ROOT/$CORPUS_DIR/skills/direct/SKILL.md"
GEPA_OUT="$HOST_ROOT/$CORPUS_DIR/skills/gepa/SKILL.md"
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/puffer-genskill-skills.XXXXXX")
trap 'rm -rf "$WORKDIR"' EXIT

common_args=(
  --provider "$PROVIDER"
  --model "$MODEL"
)

if [ -n "$EFFORT" ]; then
  common_args+=(--effort "$EFFORT")
fi

if [ "$FORCE" = "1" ] || [ ! -f "$DIRECT_OUT" ]; then
  echo "=== Direct skill for $PR ==="
  (
    cd "$WORKDIR"
    "$PUFFER_BIN_ABS" non-interactive \
      "${common_args[@]}" \
      --load-transcript "$EXPERT_MD_ABS" \
      --user-message "Generate a reusable skill based on the conversation history above. Output ONLY a SKILL.md document with YAML frontmatter (name, description) followed by sections. Stay under 15000 bytes." \
      --output "$DIRECT_OUT" \
      --transcript-out "$HOST_ROOT/$CORPUS_DIR/skills/direct/generation.jsonl" \
      --max-tool-calls "$DIRECT_TOOL_CALLS" \
      --max-tokens "$DIRECT_TOKENS"
  )
else
  echo "Direct skill already exists for $PR; set FORCE=1 to regenerate."
fi

if [ "$FORCE" = "1" ] || [ ! -f "$GEPA_OUT" ]; then
  echo "=== GEPA /genskill skill for $PR ==="
  (
    cd "$WORKDIR"
    "$PUFFER_BIN_ABS" non-interactive \
      "${common_args[@]}" \
      --load-transcript "$EXPERT_JSONL_ABS" \
      --run-command "/genskill --candidates $GENSKILL_CANDIDATES --rounds $GENSKILL_ROUNDS" \
      --output "$GEPA_OUT" \
      --transcript-out "$HOST_ROOT/$CORPUS_DIR/skills/gepa/generation.jsonl" \
      --max-tool-calls "$GENSKILL_TOOL_CALLS" \
      --max-tokens "$GENSKILL_TOKENS"
  )
else
  echo "GEPA skill already exists for $PR; set FORCE=1 to regenerate."
fi

echo "Generated $CORPUS_DIR/skills/{direct,gepa}/SKILL.md"
