#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../../.." && pwd)"
cd "$repo_root"

duration_seconds="${PUFFER_OPENROUTER_LOOP_SECONDS:-14400}"
started_at_epoch="$(date +%s)"
deadline_epoch="$((started_at_epoch + duration_seconds))"
stamp="$(date -u +%Y%m%dT%H%M%SZ)"
loop_name="${PUFFER_OPENROUTER_LOOP_NAME:-openrouter-duration-${stamp}}"
loop_dir="${PUFFER_OPENROUTER_LOOP_DIR:-apps/puffer-desktop/tests/fuzz/.runs/${loop_name}}"
summary_tsv="${loop_dir}/summary.tsv"
summary_md="${loop_dir}/summary.md"
namespace_prefix="${PUFFER_OPENROUTER_NAMESPACE_PREFIX:-openrouter-4h}"

require_env() {
  local name="$1"
  if [[ -z "${!name:-}" ]]; then
    echo "${name} is required" >&2
    exit 2
  fi
}

require_env OPENROUTER_API_KEY
require_env ANTHROPIC_BASE_URL
require_env ANTHROPIC_AUTH_TOKEN

export PUFFER_OPENROUTER_SHARD_LIMIT="${PUFFER_OPENROUTER_SHARD_LIMIT:-50}"
export PUFFER_OPENROUTER_CONCURRENCY="${PUFFER_OPENROUTER_CONCURRENCY:-10}"
export PUFFER_OPENROUTER_CASES="${PUFFER_OPENROUTER_CASES:-2}"
export PUFFER_OPENROUTER_REPLAY_LIMIT="${PUFFER_OPENROUTER_REPLAY_LIMIT:-3}"
export PUFFER_OPENROUTER_MAX_ITERATIONS="${PUFFER_OPENROUTER_MAX_ITERATIONS:-15}"
export PUFFER_OPENROUTER_MAX_STEPS="${PUFFER_OPENROUTER_MAX_STEPS:-20}"
export PUFFER_OPENROUTER_TIMEOUT_SECONDS="${PUFFER_OPENROUTER_TIMEOUT_SECONDS:-1500}"

mkdir -p "$loop_dir"
printf "round\tnamespace\tstatus\tstarted_at\tfinished_at\tshards\tcompleted_replay\tmissing_replay\treplay_cases\tactionable\tcandidates\taggregate_json\tlog\n" > "$summary_tsv"

round=1
while true; do
  now_epoch="$(date +%s)"
  if (( round > 1 && now_epoch >= deadline_epoch )); then
    break
  fi

  round_started_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  namespace="${namespace_prefix}-r$(printf '%03d' "$round")-$(date -u +%H%M%S)"
  log_path="${loop_dir}/round_$(printf '%03d' "$round").log"
  aggregate_log="${loop_dir}/round_$(printf '%03d' "$round")_aggregate.log"
  aggregate_json="apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.json"
  aggregate_md="apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign/puffer_openrouter_fuzz_report.md"

  echo "== OpenRouter fuzz round ${round}: ${namespace} =="
  echo "Started: ${round_started_at}"
  echo "Loop deadline: $(date -u -d "@${deadline_epoch}" +%Y-%m-%dT%H:%M:%SZ)"

  export PUFFER_OPENROUTER_NAMESPACE="$namespace"
  set +e
  agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py \
    --runs-dir "${loop_dir}/agentflow-runs" \
    --output summary \
    >"$log_path" 2>&1
  status=$?

  node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs \
    >"$aggregate_log" 2>&1
  aggregate_status=$?
  set -e

  round_finished_at="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
  round_json="${loop_dir}/round_$(printf '%03d' "$round")_aggregate.json"
  round_md="${loop_dir}/round_$(printf '%03d' "$round")_aggregate.md"
  if [[ -s "$aggregate_json" ]]; then
    cp "$aggregate_json" "$round_json"
  else
    echo '{"summary":{}}' > "$round_json"
  fi
  if [[ -s "$aggregate_md" ]]; then
    cp "$aggregate_md" "$round_md"
  fi

  shards="$(jq -r '.summary.shards // 0' "$round_json")"
  completed="$(jq -r '.summary.completedReplayReports // 0' "$round_json")"
  missing="$(jq -r '.summary.missingReplayReports // 0' "$round_json")"
  cases="$(jq -r '.summary.totalReplayCases // 0' "$round_json")"
  actionable="$(jq -r '.summary.actionableFailures // 0' "$round_json")"
  candidates="$(jq -r '.summary.newCandidateFindings // 0' "$round_json")"
  combined_status="$status"
  if [[ "$aggregate_status" -ne 0 ]]; then
    combined_status="${status}+aggregate-${aggregate_status}"
  fi

  printf "%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\t%s\n" \
    "$round" "$namespace" "$combined_status" "$round_started_at" "$round_finished_at" \
    "$shards" "$completed" "$missing" "$cases" "$actionable" "$candidates" "$round_json" "$log_path" \
    >> "$summary_tsv"

  {
    echo "# Puffer OpenRouter Duration Fuzz Loop"
    echo
    echo "- Loop: ${loop_name}"
    echo "- Started: $(date -u -d "@${started_at_epoch}" +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Deadline for starting new rounds: $(date -u -d "@${deadline_epoch}" +%Y-%m-%dT%H:%M:%SZ)"
    echo "- Last updated: ${round_finished_at}"
    echo "- Shard limit: ${PUFFER_OPENROUTER_SHARD_LIMIT}"
    echo "- Concurrency: ${PUFFER_OPENROUTER_CONCURRENCY}"
    echo "- Cases per shard: ${PUFFER_OPENROUTER_CASES}"
    echo "- Replay limit: ${PUFFER_OPENROUTER_REPLAY_LIMIT}"
    echo
    echo "## Rounds"
    echo
    tail -n +2 "$summary_tsv" | awk -F '\t' '{ printf "- Round %s `%s`: status=%s, replay=%s/%s, cases=%s, actionable=%s, candidates=%s\n", $1, $2, $3, $7, $6, $9, $10, $11 }'
  } > "$summary_md"

  if [[ "$status" -ne 0 ]]; then
    echo "Round ${round} exited with status ${status}; artifacts were preserved under ${loop_dir}" >&2
  fi
  if [[ "$aggregate_status" -ne 0 ]]; then
    echo "Round ${round} aggregate exited with status ${aggregate_status}; log: ${aggregate_log}" >&2
  fi

  round=$((round + 1))
done

echo "OpenRouter duration fuzz loop complete: ${summary_md}"
