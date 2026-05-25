"""Codex-planned OpenRouter campaign for Juice Shop UI-tree shards."""

from __future__ import annotations

import json
import os
from pathlib import Path

from agentflow import Graph, fanout, shell


REPO_ROOT = os.environ.get("PUFFER_REPO_ROOT") or str(Path(__file__).resolve().parents[4])
SUITE = os.environ.get(
    "PUFFER_JUICE_SHOP_SUITE",
    "apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_shards.json",
)
NAMESPACE = os.environ.get("PUFFER_JUICE_SHOP_NAMESPACE", "juice-shop-openrouter")
MODEL = os.environ.get("PUFFER_OPENROUTER_MODEL", "inclusionai/ling-2.6-flash")
PLANNER_MODEL = os.environ.get("PUFFER_OPENROUTER_PLANNER_MODEL", "gpt-5.4")
PLANNER_EFFORT = os.environ.get("PUFFER_OPENROUTER_PLANNER_EFFORT", "high")
SHARD_LIMIT = int(os.environ.get("PUFFER_JUICE_SHOP_SHARD_LIMIT", "4"))
CONCURRENCY = int(os.environ.get("PUFFER_JUICE_SHOP_CONCURRENCY", "2"))
PORT_BASE = int(os.environ.get("PUFFER_JUICE_SHOP_PORT_BASE", "13000"))
PREFLIGHT_DIR = f"apps/puffer-desktop/tests/fuzz/.runs/{NAMESPACE}-preflight"


def selected_shards():
    """Return Juice Shop shards as UI-tree fanout items."""
    suite = json.loads((Path(REPO_ROOT) / SUITE).read_text(encoding="utf-8"))
    shards = suite.get("shards", [])[:SHARD_LIMIT]
    return [
        {
            "shard_id": shard["id"],
            "area": shard.get("area", ""),
            "namespace": f"{NAMESPACE}-{shard['id']}",
            "port": PORT_BASE + index,
            "shard_path": f"juice-shop/{shard.get('area', 'area')}/{shard['id']}",
        }
        for index, shard in enumerate(shards)
    ]


SELECTED_SHARDS = selected_shards()
CLEAN_SELECTED_ARTIFACTS = "".join(
    f"rm -rf apps/puffer-desktop/tests/fuzz/.runs/{item['namespace']}\n" for item in SELECTED_SHARDS
)


PLANNER_PROMPT = f"""\
You are the Codex main planner for an OWASP Juice Shop UI/UX/security benchmark
campaign.

Repo: {REPO_ROOT}
Suite: {SUITE}
Worker model family: OpenRouter small model ({MODEL})

The UI tree is juice-shop / area / challenge-shard. Each subagent owns exactly
one shard and must not inspect unrelated shards. The native score source is
/api/Challenges before/after the bounded replay. Accessing /metrics is itself a
challenge trigger, so workers must not use it for scoring.

Produce a concise execution plan for the small explorer subagents:
- strict shard ownership rules
- false-positive filters for disabled Docker-only challenges and startup issues
- evidence/verdict/gate artifact requirements
- reminder that native score diff decides benchmark success

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
  echo "JUICE_PLAN_OK codex"
else
  echo "JUICE_PLAN_FALLBACK"
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
node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-juice-shop-explorer.mjs "${explorer_args[@]}" \
  --namespace {{ item.namespace }} \
  --suite "${PUFFER_JUICE_SHOP_SUITE:-apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_shards.json}" \
  --shard-id {{ item.shard_id }} \
  --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} \
  --out "$out_dir/worker-plan.md" \
  --json-out "$out_dir/worker-plan.json"
node apps/puffer-desktop/tests/fuzz/bin/puffer-juice-shop-runner.mjs \
  --namespace {{ item.namespace }} \
  --suite "${PUFFER_JUICE_SHOP_SUITE:-apps/puffer-desktop/tests/fuzz/benchmarks/juice_shop_shards.json}" \
  --shard-id {{ item.shard_id }} \
  --plan "$out_dir/worker-plan.json" \
  --port {{ item.port }} \
  --out "$out_dir"
echo JUICE_SHARD_OK {{ item.namespace }}
"""


with Graph(
    "juice-shop-openrouter-ui-tree-fuzz",
    description="Codex-planned small-model UI-tree fuzz campaign for OWASP Juice Shop.",
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
            f"rm -rf {PREFLIGHT_DIR} apps/puffer-desktop/tests/fuzz/.runs/juice-shop-campaign\n"
            + CLEAN_SELECTED_ARTIFACTS
            + f"mkdir -p {PREFLIGHT_DIR}\n"
            f"cp {SUITE} {PREFLIGHT_DIR}/suite.json\n"
            "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-juice-shop-explorer.mjs\n"
            "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-juice-shop-runner.mjs\n"
            "node --check apps/puffer-desktop/tests/fuzz/bin/puffer-juice-shop-aggregate.mjs\n"
            "docker image inspect ${PUFFER_JUICE_SHOP_IMAGE:-bkimminich/juice-shop} >/dev/null || docker pull ${PUFFER_JUICE_SHOP_IMAGE:-bkimminich/juice-shop}\n"
            "echo JUICE_PREFLIGHT_OK\n"
        ),
        timeout_seconds=300,
        success_criteria=[{"kind": "output_contains", "value": "JUICE_PREFLIGHT_OK"}],
    )

    plan = shell(
        task_id="plan",
        script=PLAN_SCRIPT,
        timeout_seconds=int(os.environ.get("PUFFER_OPENROUTER_PLAN_NODE_TIMEOUT_SECONDS", "260")),
        success_criteria=[{"kind": "output_contains", "value": "JUICE_PLAN"}],
    )

    run_shard = fanout(
        shell(
            task_id="run_shard",
            script=SHARD_SCRIPT,
            timeout_seconds=int(os.environ.get("PUFFER_JUICE_SHOP_TIMEOUT_SECONDS", "300")),
            success_criteria=[{"kind": "output_contains", "value": "JUICE_SHARD_OK"}],
        ),
        SELECTED_SHARDS,
    )

    aggregate = shell(
        task_id="aggregate",
        script=(
            "set -euo pipefail\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-juice-shop-aggregate.mjs "
            "--namespace ${PUFFER_JUICE_SHOP_NAMESPACE:-juice-shop-openrouter} "
            "--out apps/puffer-desktop/tests/fuzz/.runs/juice-shop-campaign\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "JUICE_SHOP_AGGREGATE_OK"}],
    )

    preflight >> plan
    plan >> run_shard
    run_shard >> aggregate


if __name__ == "__main__":
    print(dag.to_json())
