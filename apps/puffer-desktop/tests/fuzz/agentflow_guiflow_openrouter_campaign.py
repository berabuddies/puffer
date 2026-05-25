"""Codex-planned OpenRouter campaign for GUIFlow benchmark UI-tree shards."""

from __future__ import annotations

import json
import os
from pathlib import Path

from agentflow import Graph, fanout, shell


REPO_ROOT = os.environ.get("PUFFER_REPO_ROOT") or str(Path(__file__).resolve().parents[4])
GUIFLOW_ROOT = os.environ.get("PUFFER_GUIFLOW_ROOT", str(Path(REPO_ROOT).parent / "guiflow-paper"))
SUITE = os.environ.get("PUFFER_GUIFLOW_SUITE", f"{GUIFLOW_ROOT}/benchmarks/webtestbench_subset.json")
NAMESPACE = os.environ.get("PUFFER_GUIFLOW_NAMESPACE", "guiflow-openrouter")
MODEL = os.environ.get("PUFFER_OPENROUTER_MODEL", "inclusionai/ling-2.6-flash")
PLANNER_MODEL = os.environ.get("PUFFER_OPENROUTER_PLANNER_MODEL", "gpt-5.4")
PLANNER_EFFORT = os.environ.get("PUFFER_OPENROUTER_PLANNER_EFFORT", "high")
SHARD_LIMIT = int(os.environ.get("PUFFER_GUIFLOW_SHARD_LIMIT", "6"))
CONCURRENCY = int(os.environ.get("PUFFER_GUIFLOW_CONCURRENCY", "3"))
PREFLIGHT_DIR = f"apps/puffer-desktop/tests/fuzz/.runs/{NAMESPACE}-preflight"


def selected_cases():
    """Return benchmark cases as UI-tree shard items."""
    suite = json.loads(Path(SUITE).read_text(encoding="utf-8"))
    cases = suite.get("cases", [])[:SHARD_LIMIT]
    return [
        {
            "case_id": case["id"],
            "app": case.get("app", ""),
            "gold_bug": str(bool(case.get("gold_bug", False))).lower(),
            "namespace": f"{NAMESPACE}-{case['id']}",
            "shard_path": f"benchmark/{suite.get('name', 'suite')}/{case.get('app', 'app')}/{case['id']}",
        }
        for case in cases
    ]


SELECTED_CASES = selected_cases()
CLEAN_SELECTED_ARTIFACTS = "".join(
    f"rm -rf apps/puffer-desktop/tests/fuzz/.runs/{case['namespace']}\n" for case in SELECTED_CASES
)


PLANNER_PROMPT = f"""\
You are the Codex main planner for a GUIFlow benchmark UI/UX fuzz campaign.

Repo: {REPO_ROOT}
Benchmark root: {GUIFLOW_ROOT}
Suite: {SUITE}
Worker model family: OpenRouter small model ({MODEL})

The UI tree is benchmark / app / case / oracle. Each subagent owns exactly one
case shard and must not inspect unrelated benchmark cases.

Produce a concise execution plan for the small explorer subagents:
- strict shard ownership rules
- benchmark fixture risks
- false-positive filters
- evidence/verdict/gate artifact requirements
- reminder that deterministic replay and citation gate decide admission

Do not modify files.
"""


PLAN_SCRIPT = f"""\
set -euo pipefail
preflight_dir="{PREFLIGHT_DIR}"
mkdir -p "$preflight_dir"
prompt_file="$preflight_dir/planner-prompt.txt"
output_file="$preflight_dir/planner-output.md"
transcript_file="$preflight_dir/planner-transcript.log"
error_file="$preflight_dir/planner-error.log"
cat > "$prompt_file" <<'PLANNER_PROMPT_EOF'
{PLANNER_PROMPT}
PLANNER_PROMPT_EOF
planner_timeout="${{PUFFER_OPENROUTER_CODEX_PLAN_TIMEOUT_SECONDS:-180}}"
if timeout "$planner_timeout" codex exec \
  --model "${{PUFFER_OPENROUTER_PLANNER_MODEL:-gpt-5.4}}" \
  --sandbox read-only \
  -c model_reasoning_effort="${{PUFFER_OPENROUTER_PLANNER_EFFORT:-high}}" \
  --output-last-message "$output_file" \
  - \
  < "$prompt_file" \
  > "$transcript_file" 2> "$error_file" && [[ -s "$output_file" ]]; then
  cat "$output_file"
  echo
  echo "GUIFLOW_PLAN_OK codex"
else
  echo "GUIFLOW_PLAN_FALLBACK"
  if [[ -s "$error_file" ]]; then
    tail -n 20 "$error_file"
  fi
  cat "$prompt_file"
fi
"""


SHARD_SCRIPT = """\
set -euo pipefail
out_dir="apps/puffer-desktop/tests/fuzz/.runs/{{ item.namespace }}"
mkdir -p "$out_dir"
cat > "$out_dir/planner.md" <<'PLANNER_EOF'
{{ nodes.plan.output }}
PLANNER_EOF
explorer_args=()
if [[ "${PUFFER_OPENROUTER_OFFLINE_SMOKE:-0}" == "1" ]]; then
  explorer_args+=(--offline)
fi
node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-benchmark-explorer.mjs "${explorer_args[@]}" \
  --namespace {{ item.namespace }} \
  --suite "${PUFFER_GUIFLOW_SUITE}" \
  --case-id {{ item.case_id }} \
  --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} \
  --out "$out_dir/worker-plan.md" \
  --json-out "$out_dir/worker-plan.json"
node apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs \
  --root "${PUFFER_GUIFLOW_ROOT}" \
  --suite "${PUFFER_GUIFLOW_SUITE}" \
  --case-id {{ item.case_id }} \
  --no-gold-assert \
  --out "$out_dir"
echo GUIFLOW_SHARD_OK {{ item.namespace }}
"""


with Graph(
    "guiflow-benchmark-openrouter-ui-tree-fuzz",
    description="Codex-planned small-model UI-tree fuzz campaign for GUIFlow benchmark cases.",
    working_dir=REPO_ROOT,
    concurrency=CONCURRENCY,
    fail_fast=False,
    node_defaults={"capture": "final", "retries": 0},
) as dag:
    preflight = shell(
        task_id="preflight",
        script=(
            "set -euo pipefail\n"
            "test -n \"${OPENROUTER_API_KEY:-}\" || test -s \"${PUFFER_OPENROUTER_API_KEY_FILE:-/dev/null}\" || test \"${PUFFER_OPENROUTER_OFFLINE_SMOKE:-0}\" = \"1\"\n"
            f"rm -rf {PREFLIGHT_DIR} apps/puffer-desktop/tests/fuzz/.runs/guiflow-benchmark-campaign\n"
            + CLEAN_SELECTED_ARTIFACTS
            + f"mkdir -p {PREFLIGHT_DIR}\n"
            f"cp {SUITE} {PREFLIGHT_DIR}/suite.json\n"
            "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-benchmark-explorer.mjs\n"
            "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs\n"
            "echo GUIFLOW_PREFLIGHT_OK\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "GUIFLOW_PREFLIGHT_OK"}],
    )

    plan = shell(
        task_id="plan",
        script=PLAN_SCRIPT,
        timeout_seconds=int(os.environ.get("PUFFER_OPENROUTER_PLAN_NODE_TIMEOUT_SECONDS", "180")),
        success_criteria=[{"kind": "output_contains", "value": "GUIFLOW_PLAN"}],
    )

    run_shard = fanout(
        shell(
            task_id="run_shard",
            script=SHARD_SCRIPT,
            timeout_seconds=int(os.environ.get("PUFFER_GUIFLOW_TIMEOUT_SECONDS", "900")),
            success_criteria=[{"kind": "output_contains", "value": "GUIFLOW_SHARD_OK"}],
        ),
        SELECTED_CASES,
    )

    aggregate = shell(
        task_id="aggregate",
        script=(
            "set -euo pipefail\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-benchmark-aggregate.mjs "
            "--namespace ${PUFFER_GUIFLOW_NAMESPACE:-guiflow-openrouter} "
            "--out apps/puffer-desktop/tests/fuzz/.runs/guiflow-benchmark-campaign\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "GUIFLOW_BENCHMARK_AGGREGATE_OK"}],
    )

    preflight >> plan
    plan >> run_shard
    run_shard >> aggregate


if __name__ == "__main__":
    print(dag.to_json())
