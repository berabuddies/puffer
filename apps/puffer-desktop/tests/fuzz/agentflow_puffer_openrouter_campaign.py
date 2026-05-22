"""Claude-planned OpenRouter small-model campaign for Puffer UI/UX fuzzing.

This campaign is intentionally separate from the full Claude/Infer campaign.
Claude Opus acts as the main planner, and cheaper OpenAI-compatible workers
only execute bounded UI-tree shards and report trigger evidence.

Required environment:

  export OPENROUTER_API_KEY="<key>"
  export ANTHROPIC_BASE_URL="https://api-infer.agentsey.ai"
  export ANTHROPIC_AUTH_TOKEN="<infer-key>"
  export ANTHROPIC_API_KEY=""

Optional controls:

  export PUFFER_OPENROUTER_PLANNER_MODEL="claude-opus-4-6"
  export PUFFER_OPENROUTER_MODEL="inclusionai/ling-2.6-flash"
  export PUFFER_OPENROUTER_CONCURRENCY=2
  export PUFFER_OPENROUTER_SHARD_LIMIT=2
  export PUFFER_OPENROUTER_AREAS="chat-composer-send,settings-mcp"

Run:

  agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py \
    --runs-dir apps/puffer-desktop/tests/fuzz/.runs/openrouter-local-runs \
    --output summary

The planner does not execute GUI fuzzing. Workers must not plan globally, patch
product code, commit, push, or edit BUGS.md directly. They only write artifacts
under apps/puffer-desktop/tests/fuzz/.runs/.
"""

from __future__ import annotations

import json
import os
import subprocess

from agentflow import Graph, claude, fanout, shell


REPO_ROOT = "/dsk/hdd/home/llmft/Riema/puffer"
TASK_PATH = "apps/puffer-desktop/tests/fuzz/prompt.txt"
MODEL = os.environ.get("PUFFER_OPENROUTER_MODEL", "inclusionai/ling-2.6-flash")
NAMESPACE = os.environ.get("PUFFER_OPENROUTER_NAMESPACE", "openrouter-small")
SHARD_LIMIT = os.environ.get("PUFFER_OPENROUTER_SHARD_LIMIT", "2")
CONCURRENCY = int(os.environ.get("PUFFER_OPENROUTER_CONCURRENCY", "2"))
PLANNER_MODEL = os.environ.get("PUFFER_OPENROUTER_PLANNER_MODEL", "claude-opus-4-6")

CLAUDE_PLANNER_ENV = {
    "ANTHROPIC_API_KEY": "",
    "ANTHROPIC_MODEL": PLANNER_MODEL,
}


def scheduled_areas():
    """Return scheduler-selected shards for the small-model run."""
    requested = [
        item.strip()
        for item in os.environ.get("PUFFER_OPENROUTER_AREAS", "").split(",")
        if item.strip()
    ]
    command = [
        "node",
        "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
        "schedule",
        "--limit",
        SHARD_LIMIT,
        "--namespace",
        NAMESPACE,
        "--format",
        "json",
    ]
    if requested:
        command.extend(["--shards", ",".join(requested)])
    payload = subprocess.check_output(command, cwd=REPO_ROOT, text=True)
    schedule = json.loads(payload)
    return [
        {
            "name": item["shardId"],
            "seed": item["seed"],
            "iterations": min(int(item["iterations"]), int(os.environ.get("PUFFER_OPENROUTER_MAX_ITERATIONS", "10"))),
            "steps": min(int(item["steps"]), int(os.environ.get("PUFFER_OPENROUTER_MAX_STEPS", "12"))),
            "replay_limit": min(int(item["replayLimit"]), int(os.environ.get("PUFFER_OPENROUTER_REPLAY_LIMIT", "2"))),
            "priority": f"score {item['score']}",
            "focus": item["title"],
            "start_node": item["startNode"],
            "entrypoint": item["entrypoint"],
            "owned_nodes": ", ".join(item["ownedNodes"]),
            "allowed_setup_nodes": ", ".join(item["allowedSetupNodes"]),
            "allowed_async_events": ", ".join(item["allowedAsyncEvents"]),
            "invariants": ", ".join(item["invariants"]),
            "namespace": f"{NAMESPACE}-{item['shardId']}",
        }
        for item in schedule["items"]
    ]


SELECTED_AREAS = scheduled_areas()
CLEAN_SELECTED_ARTIFACTS = "".join(
    f"rm -rf apps/puffer-desktop/tests/fuzz/.runs/{area['namespace']}\n" for area in SELECTED_AREAS
)


PLANNER_PROMPT = f"""\
You are the Claude Opus main planner for a small-model Puffer UI/UX fuzz smoke
campaign.

Repo: {REPO_ROOT}
Task file: {TASK_PATH}
Worker model family: OpenRouter small model ({MODEL})

Read only these files:
- {TASK_PATH}
- apps/puffer-desktop/tests/fuzz/README.md
- apps/puffer-desktop/tests/fuzz/agent_guide.md
- apps/puffer-desktop/tests/fuzz/playwright_adapter.md
- apps/puffer-desktop/tests/fuzz/BUGS.md

Produce a short execution plan for small-model worker shards:
- strict scope boundaries
- likely false-positive patterns
- exact report format requirements
- reminder that workers must output BUG_LIST_APPEND blocks, not edit BUGS.md
- reminder that workers should execute the fixed commands instead of
  improvising campaign strategy

Do not modify files.
"""


SHARD_SCRIPT = """\
set -euo pipefail
out_dir="apps/puffer-desktop/tests/fuzz/.runs/{{ item.namespace }}"
mkdir -p "$out_dir"
cat > "$out_dir/planner.md" <<'PLANNER_EOF'
{{ nodes.plan.output }}
PLANNER_EOF
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed {{ item.seed }} --iterations {{ item.iterations }} --steps {{ item.steps }} --rng-seed {{ item.namespace }} --out "$out_dir/run.json"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input "$out_dir/run.json" --out "$out_dir/report.md"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input "$out_dir/run.json" --shard {{ item.name }} --limit {{ item.replay_limit }} --out "$out_dir/top.json" --report-out "$out_dir/top.md"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds {{ item.seed }} --shard {{ item.name }} --limit {{ item.replay_limit }} --attempts 2 --timeout 120 --rng-seed {{ item.namespace }} --namespace {{ item.namespace }} --fail-on-new-finding
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard {{ item.name }} --input "$out_dir/bounded-replay-report.json" --namespace {{ item.namespace }}
node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/findings.md"
echo OPENROUTER_SHARD_OK {{ item.namespace }}
"""


with Graph(
    "puffer-uiux-openrouter-small-fuzz",
    description="Small-model OpenRouter smoke campaign for Puffer UI/UX fuzz shards.",
    working_dir=REPO_ROOT,
    concurrency=CONCURRENCY,
    fail_fast=False,
    node_defaults={
        "capture": "final",
        "retries": 0,
    },
    agent_defaults={
        "claude": {
            "model": PLANNER_MODEL,
            "env": CLAUDE_PLANNER_ENV,
            "timeout_seconds": int(os.environ.get("PUFFER_OPENROUTER_PLANNER_TIMEOUT_SECONDS", "900")),
        },
    },
) as dag:
    preflight = shell(
        task_id="preflight",
        script=(
            "set -euo pipefail\n"
            "test -n \"${OPENROUTER_API_KEY:-}\"\n"
            "test -n \"${ANTHROPIC_BASE_URL:-}\"\n"
            "test -n \"${ANTHROPIC_AUTH_TOKEN:-}\"\n"
            "rm -rf apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign\n"
            + CLEAN_SELECTED_ARTIFACTS
            + "mkdir -p apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit ${PUFFER_OPENROUTER_SHARD_LIMIT:-2} --namespace ${PUFFER_OPENROUTER_NAMESPACE:-openrouter-small} --out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/schedule.md --json-out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/schedule.json\n"
            "echo OPENROUTER_PREFLIGHT_OK\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "OPENROUTER_PREFLIGHT_OK"}],
    )

    plan = claude(
        task_id="plan",
        prompt=PLANNER_PROMPT,
        tools="read_only",
        timeout_seconds=int(os.environ.get("PUFFER_OPENROUTER_PLANNER_TIMEOUT_SECONDS", "900")),
    )

    run_shard = fanout(
        shell(
            task_id="run_shard",
            script=SHARD_SCRIPT,
            timeout_seconds=int(os.environ.get("PUFFER_OPENROUTER_TIMEOUT_SECONDS", "1200")),
            success_criteria=[{"kind": "output_contains", "value": "OPENROUTER_SHARD_OK"}],
        ),
        SELECTED_AREAS,
    )

    aggregate_findings = shell(
        task_id="aggregate_findings",
        script=(
            "set -euo pipefail\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-aggregate.mjs\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "OPENROUTER_AGGREGATE_OK"}],
    )

    preflight >> plan
    plan >> run_shard
    run_shard >> aggregate_findings


if __name__ == "__main__":
    print(dag.to_json())
