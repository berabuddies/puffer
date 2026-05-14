#!/usr/bin/env bash
# Generates direct and GEPA skills for one PR from its reference fix.
#
# Usage: generate_skills.sh pr-12345

set -euo pipefail

PR="${1:?usage: generate_skills.sh <pr-id>}"
ROOT="benchmark/genskill/ladybird"
CORPUS_DIR="$ROOT/pr_corpus/$PR"
META="$CORPUS_DIR/meta.json"
REFERENCE_PATCH="$CORPUS_DIR/reference_fix.patch"
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

[ -f "$META" ] || { echo "no meta.json at $META" >&2; exit 1; }
[ -f "$REFERENCE_PATCH" ] || { echo "no reference_fix.patch at $REFERENCE_PATCH" >&2; exit 1; }
[ -x "$PUFFER_BIN" ] || { echo "missing executable puffer at $PUFFER_BIN" >&2; exit 1; }

mkdir -p "$CORPUS_DIR/skills/direct" "$CORPUS_DIR/skills/gepa"

HOST_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || pwd)
PUFFER_BIN_ABS="$(cd "$(dirname "$PUFFER_BIN")" && pwd -P)/$(basename "$PUFFER_BIN")"
META_ABS="$HOST_ROOT/$META"
REFERENCE_PATCH_ABS="$HOST_ROOT/$REFERENCE_PATCH"
DIRECT_OUT="$HOST_ROOT/$CORPUS_DIR/skills/direct/SKILL.md"
GEPA_OUT="$HOST_ROOT/$CORPUS_DIR/skills/gepa/SKILL.md"
GEPA_FALLBACK_MARKER="$HOST_ROOT/$CORPUS_DIR/skills/gepa/FALLBACK_TO_DIRECT"
WORKDIR=$(mktemp -d "${TMPDIR:-/tmp}/puffer-genskill-skills.XXXXXX")
trap 'rm -rf "$WORKDIR"' EXIT
PATCH_CONTEXT="$WORKDIR/reference-solution-context.txt"
PATCH_AWARE_MD="$WORKDIR/reference-solution.md"

common_args=(
  --provider "$PROVIDER"
  --model "$MODEL"
)

if [ -n "$EFFORT" ]; then
  common_args+=(--effort "$EFFORT")
fi

{
  echo "Reference solution context for $PR"
  echo
  echo "This context is allowed only during skill generation. The replay agent"
  echo "must not see this patch directly; it will see only the generated SKILL.md."
  echo
  echo "PR metadata:"
  jq . "$META_ABS"
  echo
  echo "Reference fix patch:"
  echo '```diff'
  cat "$REFERENCE_PATCH_ABS"
  echo '```'
  echo
  echo "When generating the skill, extract reusable debugging knowledge from the"
  echo "reference fix. The skill must summarize:"
  echo "- bug root cause"
  echo "- key files, functions, classes, and symbols"
  echo "- what the failing test is proving"
  echo "- the correct fix shape without pasting the whole patch"
  echo "- common wrong fixes or tempting dead ends"
  echo
  echo "The skill must help a replay agent independently rediscover the fix while"
  echo "remaining under 15000 bytes."
} > "$PATCH_CONTEXT"

{
  cat "$PATCH_CONTEXT"
} > "$PATCH_AWARE_MD"

if [ "$FORCE" = "1" ] || [ ! -f "$DIRECT_OUT" ]; then
  echo "=== Direct skill for $PR ==="
  (
    cd "$WORKDIR"
    "$PUFFER_BIN_ABS" non-interactive \
      "${common_args[@]}" \
      --load-transcript "$PATCH_AWARE_MD" \
      --user-message "Generate a reusable bug-fixing SKILL.md from the reference solution context above. Output ONLY a SKILL.md document with YAML frontmatter (name, description) followed by sections. Include bug root cause, key files/functions, failing-test meaning, correct fix shape, and common wrong fixes. Do not paste the whole reference patch; distill it into reusable guidance. Stay under 15000 bytes." \
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
      --load-transcript "$PATCH_AWARE_MD" \
      --user-message "$(cat "$PATCH_CONTEXT")" \
      --run-command "/genskill --candidates $GENSKILL_CANDIDATES --rounds $GENSKILL_ROUNDS" \
      --output "$GEPA_OUT" \
      --transcript-out "$HOST_ROOT/$CORPUS_DIR/skills/gepa/generation.jsonl" \
      --max-tool-calls "$GENSKILL_TOOL_CALLS" \
      --max-tokens "$GENSKILL_TOKENS"
  )
  if grep -q "/genskill needs a substantive transcript" "$GEPA_OUT" \
    || [ "$(wc -c < "$GEPA_OUT" | tr -d ' ')" -lt 500 ]; then
    echo "GEPA did not produce a substantive skill for $PR; falling back to direct skill." >&2
    cp "$DIRECT_OUT" "$GEPA_OUT"
    {
      echo "fallback_to_direct=true"
      echo "reason=genskill_non_substantive_transcript"
      echo "generated_at=$(date -u +"%Y-%m-%dT%H:%M:%SZ")"
    } > "$GEPA_FALLBACK_MARKER"
  else
    rm -f "$GEPA_FALLBACK_MARKER"
  fi
else
  echo "GEPA skill already exists for $PR; set FORCE=1 to regenerate."
fi

echo "Generated $CORPUS_DIR/skills/{direct,gepa}/SKILL.md"
