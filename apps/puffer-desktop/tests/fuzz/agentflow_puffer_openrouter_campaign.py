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
from pathlib import Path

from agentflow import Graph, fanout, shell


REPO_ROOT = os.environ.get("PUFFER_REPO_ROOT") or str(Path(__file__).resolve().parents[4])
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
    target_count = max(1, int(SHARD_LIMIT))
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
    base_items = [
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
    if not base_items:
        return []
    if len(base_items) >= target_count:
        return base_items[:target_count]
    expanded = []
    for index in range(target_count):
        item = dict(base_items[index % len(base_items)])
        round_index = index // len(base_items)
        if round_index > 0:
            item["namespace"] = f"{NAMESPACE}-r{round_index:02d}-{item['name']}"
            item["priority"] = f"{item['priority']} replica {round_index}"
        expanded.append(item)
    return expanded


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
- apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/prompt-evolution.md
- apps/puffer-desktop/tests/fuzz/playwright_adapter.md
- apps/puffer-desktop/tests/fuzz/BUGS.md

Produce a short execution plan for small-model worker shards:
- strict scope boundaries
- likely false-positive patterns
- prompt-evolution checklist items that workers must obey
- exact report format requirements
- reminder that workers must output BUG_LIST_APPEND blocks, not edit BUGS.md
- reminder that workers should execute the fixed commands instead of
  improvising campaign strategy

Do not modify files.
"""


FALLBACK_PLANNER_TEXT = f"""\
# OpenRouter Small-Model UI/UX Fuzz Fallback Plan

Claude Opus planner guidance was unavailable for this round. Continue with this
deterministic fallback instead of skipping shard execution.

## Scope Boundaries

- Each worker owns only its scheduled shard and the `ownedNodes` recorded in the
  preflight schedule.
- `allowedSetupNodes` may be used only to reach the shard start node.
- Out-of-shard observations must be reported as routing notes, not accepted
  findings.
- Prioritize core user loops first: chat composer, turn lifecycle, session
  switching, permission/question flows, transcript reload, new-agent creation,
  and provider/model selection.
- Secondary panes such as Browser, Files, Terminal, Settings, Pipelines, and
  Workspace are valid when the scheduler assigns them.

## False-Positive Filters

- Reject missing local dependencies, missing auth, missing browser binary,
  network failures, and fake-daemon fixture gaps.
- Reject cosmetic layout/copy issues unless they block or corrupt interaction.
- Reject generated candidates without bounded replay evidence.
- Reject known duplicates from the replay report or BUGS ledger.
- Reject disabled controls when a visible recovery path exists.
- Reject timeouts that do not leave a product-visible stuck or corrupted state.

## Worker Checklist

- Generate candidate UI paths only inside the assigned shard.
- Combine one visible user action with one async stressor when possible:
  late success, late failure, duplicate submit, reconnect, stale event, reload,
  or rapid session switch.
- Keep candidates materially different by varying the control, timing, or state
  transition.
- Run the fixed command sequence from the shard script. Do not patch product
  code, commit, push, or edit BUGS.md.
- Promote a finding only when bounded replay provides stable evidence and the
  issue blocks, duplicates, loses, corrupts, or misroutes a user-visible result.

## Accepted Finding Format

Accepted findings must include title, severity, area/component, seed, replay
case ID, minimal trigger steps, expected behavior, actual behavior, user impact,
why this is a product bug, shard ownership, stability, likely source area,
regression test target, and artifact paths.

Accepted findings must include a BUG_LIST_APPEND block. Workers must not edit
apps/puffer-desktop/tests/fuzz/BUGS.md directly.
"""


PLAN_SCRIPT = f"""\
set -euo pipefail
preflight_dir="apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight"
mkdir -p "$preflight_dir"
prompt_file="$preflight_dir/planner-prompt.txt"
fallback_file="$preflight_dir/fallback-plan.md"
output_file="$preflight_dir/planner-output.md"
error_file="$preflight_dir/planner-error.log"
cat > "$prompt_file" <<'PLANNER_PROMPT_EOF'
{PLANNER_PROMPT}
PLANNER_PROMPT_EOF
cat > "$fallback_file" <<'FALLBACK_PLAN_EOF'
{FALLBACK_PLANNER_TEXT}
FALLBACK_PLAN_EOF
planner_timeout="${{PUFFER_OPENROUTER_CLAUDE_PLAN_TIMEOUT_SECONDS:-120}}"
if timeout "$planner_timeout" claude \
  --model "${{PUFFER_OPENROUTER_PLANNER_MODEL:-claude-opus-4-6}}" \
  --print \
  --permission-mode plan \
  --tools Read \
  < "$prompt_file" \
  > "$output_file" 2> "$error_file" && [[ -s "$output_file" ]]; then
  cat "$output_file"
  echo
  echo "OPENROUTER_PLAN_OK claude"
else
  echo "OPENROUTER_PLAN_FALLBACK"
  if [[ -s "$error_file" ]]; then
    echo
    echo "Claude planner error excerpt:"
    tail -n 20 "$error_file"
    echo
  fi
  cat "$fallback_file"
fi
"""


SHARD_SCRIPT = """\
set -euo pipefail
out_dir="apps/puffer-desktop/tests/fuzz/.runs/{{ item.namespace }}"
mkdir -p "$out_dir"
cp apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/prompt-evolution.md "$out_dir/prompt-evolution.md"
cat > "$out_dir/planner.md" <<'PLANNER_EOF'
{{ nodes.plan.output }}
PLANNER_EOF
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate
node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-explorer.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --steps {{ item.steps }} --cases ${PUFFER_OPENROUTER_CASES:-1} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/run.json"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input "$out_dir/run.json" --out "$out_dir/report.md"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input "$out_dir/run.json" --shard {{ item.name }} --limit {{ item.replay_limit }} --out "$out_dir/top.json" --report-out "$out_dir/top.md"
set +e
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --input "$out_dir/run.json" --seeds {{ item.seed }} --shard {{ item.name }} --limit {{ item.replay_limit }} --attempts 2 --timeout 120 --rng-seed {{ item.namespace }} --namespace {{ item.namespace }} --fail-on-new-finding
replay_status=$?
set -e
echo OPENROUTER_REPLAY_STATUS "$replay_status"
if [[ -s "$out_dir/bounded-replay-report.json" ]]; then
  node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard {{ item.name }} --input "$out_dir/bounded-replay-report.json" --namespace {{ item.namespace }}
  node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/findings.md"
else
  echo OPENROUTER_REPLAY_REPORT_MISSING {{ item.namespace }}
fi
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
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-prompt --out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/prompt-evolution.md --json-out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/prompt-evolution.json\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit ${PUFFER_OPENROUTER_SHARD_LIMIT:-2} --namespace ${PUFFER_OPENROUTER_NAMESPACE:-openrouter-small} --out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/schedule.md --json-out apps/puffer-desktop/tests/fuzz/.runs/openrouter-preflight/schedule.json\n"
            "echo OPENROUTER_PREFLIGHT_OK\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "OPENROUTER_PREFLIGHT_OK"}],
    )

    plan = shell(
        task_id="plan",
        script=PLAN_SCRIPT,
        timeout_seconds=int(os.environ.get("PUFFER_OPENROUTER_PLAN_NODE_TIMEOUT_SECONDS", "180")),
        success_criteria=[{"kind": "output_contains", "value": "OPENROUTER_PLAN"}],
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
