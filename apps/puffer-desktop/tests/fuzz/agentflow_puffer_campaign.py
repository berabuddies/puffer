"""AgentFlow campaign for Puffer UI/UX fuzz bug hunting.

Run from the Puffer repo root after exporting the Infer-backed Claude
environment:

  export INFER_API_KEY="<redacted>"
  export ANTHROPIC_BASE_URL="https://api-infer.agentsey.ai"
  export ANTHROPIC_AUTH_TOKEN="$INFER_API_KEY"
  export ANTHROPIC_API_KEY=""
  export ANTHROPIC_MODEL="claude-opus-4-6"
  agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_campaign.py \
    --runs-dir apps/puffer-desktop/tests/fuzz/.runs/agentflow-local-runs \
    --output summary

The graph intentionally does not commit, push, or patch product code. Workers
may create temporary replay specs under `apps/puffer-desktop/tests/fuzz/.runs/`; the final aggregate is a
local deterministic report so campaign completion does not depend on a large
Claude merge context.
"""

import json
import os
import subprocess
from pathlib import Path

from agentflow import Graph, claude, fanout, shell


REPO_ROOT = os.environ.get("PUFFER_REPO_ROOT") or str(Path(__file__).resolve().parents[4])
TASK_PATH = "apps/puffer-desktop/tests/fuzz/prompt.txt"
MODEL = "claude-opus-4-6"

# Keep ANTHROPIC_API_KEY explicitly empty for the Infer Claude gateway. The
# base URL and auth token are inherited from the launching shell so secrets do
# not appear in this campaign file.
CLAUDE_ENV = {
    "ANTHROPIC_API_KEY": "",
    "ANTHROPIC_MODEL": MODEL,
}

LEGACY_AREAS = [
    {
        "name": "chat-turn-lifecycle",
        "seed": "chat-turn-race",
        "iterations": 36,
        "steps": 20,
        "replay_limit": 3,
        "priority": "P0/P1",
        "focus": (
            "chat/session/new agent/turn/reload core loop: send, stop, streaming, "
            "permission, question, session switch, transcript reload, stale events, "
            "one request per intent, and draft preservation"
        ),
    },
    {
        "name": "workspace-session-switching",
        "seed": "workspace-session-race",
        "iterations": 32,
        "steps": 18,
        "replay_limit": 3,
        "priority": "P1",
        "focus": (
            "workspace board, project grouping, search/filter, session selection, "
            "active-agent sidebar, pin state, reconnect, and stale refresh"
        ),
    },
    {
        "name": "provider-auth-model",
        "seed": "provider-auth-model-race",
        "iterations": 32,
        "steps": 18,
        "replay_limit": 3,
        "priority": "P1",
        "focus": (
            "provider auth import/refresh, default model save, new-agent provider "
            "selection, provider/model mismatch, stale model list responses"
        ),
    },
    {
        "name": "modal-focus-keyboard",
        "seed": "modal-focus-race",
        "iterations": 40,
        "steps": 3,
        "replay_limit": 3,
        "priority": "P1",
        "focus": (
            "New agent, Create Project, and Switch workspace modal initial focus, "
            "Tab containment, keyboard-only navigation, and modal recovery"
        ),
    },
    {
        "name": "files-terminal",
        "seed": "files-terminal-race",
        "iterations": 24,
        "steps": 16,
        "replay_limit": 2,
        "priority": "P1/P2",
        "focus": (
            "Files and Terminal secondary workflows: dirty draft preservation, save "
            "failure/reload races, pty focus/close/input routing, stale daemon events"
        ),
    },
    {
        "name": "browser-tabs-input",
        "seed": "browser-tab-race",
        "iterations": 24,
        "steps": 16,
        "replay_limit": 2,
        "priority": "P1/P2",
        "focus": (
            "Browser pane secondary workflows: tab open/close/focus, address input, "
            "navigation failures, stale frames, page keyboard input, and tab recovery"
        ),
    },
    {
        "name": "settings-connectors",
        "seed": "settings-connector-race",
        "iterations": 20,
        "steps": 14,
        "replay_limit": 2,
        "priority": "P2",
        "focus": (
            "Settings connector setup: deterministic /connect commands, dynamic "
            "AskUserQuestion inputs, stale snapshots, and narrow layouts"
        ),
    },
    {
        "name": "settings-mcp-permissions",
        "seed": "settings-mcp-permission-race",
        "iterations": 20,
        "steps": 14,
        "replay_limit": 2,
        "priority": "P2",
        "focus": (
            "Settings MCP and permission editors: add/update/remove/test, save races, "
            "stale refresh, validation, and draft preservation"
        ),
    },
    {
        "name": "workflows-drafts",
        "seed": "workflows-draft-race",
        "iterations": 20,
        "steps": 14,
        "replay_limit": 2,
        "priority": "P2",
        "focus": (
            "Workflow editor drafts and graph state: tab switching, refresh, provider "
            "changes, trigger edits, required-field validation, and cycle handling"
        ),
    },
]
AREA_BY_NAME = {area["name"]: area for area in LEGACY_AREAS}
AREA_BY_SEED = {area["seed"]: area for area in LEGACY_AREAS}


def scheduled_areas(requested):
    """Return scheduler-selected UI tree shards for this AgentFlow run."""
    command = [
        "node",
        "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
        "schedule",
        "--limit",
        os.environ.get("PUFFER_AGENTFLOW_SHARD_LIMIT", "4"),
        "--namespace",
        os.environ.get("PUFFER_AGENTFLOW_NAMESPACE", "agentflow-scheduled"),
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
            "iterations": item["iterations"],
            "steps": item["steps"],
            "replay_limit": item["replayLimit"],
            "priority": f"score {item['score']}",
            "focus": item["title"],
            "start_node": item["startNode"],
            "entrypoint": item["entrypoint"],
            "owned_nodes": ", ".join(item["ownedNodes"]),
            "allowed_setup_nodes": ", ".join(item["allowedSetupNodes"]),
            "allowed_async_events": ", ".join(item["allowedAsyncEvents"]),
            "invariants": ", ".join(item["invariants"]),
        }
        for item in schedule["items"]
    ]


def selected_areas():
    """Return the campaign shards selected for this run."""
    raw = os.environ.get("PUFFER_AGENTFLOW_AREAS", "")
    requested = [item.strip() for item in raw.split(",") if item.strip()]
    if os.environ.get("PUFFER_AGENTFLOW_LEGACY_AREAS") != "1":
        return scheduled_areas(requested)
    if not requested:
        return LEGACY_AREAS
    selected = []
    missing = []
    for item in requested:
        area = AREA_BY_NAME.get(item) or AREA_BY_SEED.get(item)
        if area is None:
            missing.append(item)
        else:
            selected.append(area)
    if missing:
        raise ValueError(f"Unknown PUFFER_AGENTFLOW_AREAS item(s): {', '.join(missing)}")
    return selected


SELECTED_AREAS = selected_areas()
CLEAN_SELECTED_ARTIFACTS = "".join(
    f"rm -rf apps/puffer-desktop/tests/fuzz/.runs/agentflow-{area['name']}\n" for area in SELECTED_AREAS
)


PLANNER_PROMPT = f"""\
You are planning a Puffer UI/UX fuzz campaign using AgentFlow.

Repo: {REPO_ROOT}
Task file: {TASK_PATH}

Read:
- {TASK_PATH}
- apps/puffer-desktop/tests/fuzz/README.md
- apps/puffer-desktop/tests/fuzz/agent_guide.md
- apps/puffer-desktop/tests/fuzz/playwright_adapter.md
- apps/puffer-desktop/tests/fuzz/manifests/puffer-ui.json
- apps/puffer-desktop/tests/fuzz/coverage-ledger.json

Then produce a concise campaign plan for the worker shards:
- priority order
- known already-confirmed findings to avoid duplicating unless used as a harness sanity check
- coverage gaps workers should target
- false-positive rules
- report format requirements

Do not modify files. Do not run product fixes. The output should be directly useful
to the worker shards and reducers.
"""


SHARD_PROMPT = f"""\
You are a Puffer UI/UX fuzz shard running inside an AgentFlow campaign.

Repo: {REPO_ROOT}
""" + """\
Task file: apps/puffer-desktop/tests/fuzz/prompt.txt
Area: {{ item.name }}
Seed: {{ item.seed }}
Priority target: {{ item.priority }}
Focus: {{ item.focus }}
Start node: {{ item.start_node }}
Entrypoint: {{ item.entrypoint }}
Owned nodes: {{ item.owned_nodes }}
Allowed setup nodes: {{ item.allowed_setup_nodes }}
Allowed async events: {{ item.allowed_async_events }}
Required invariants: {{ item.invariants }}
Iterations: {{ item.iterations }}
Steps: {{ item.steps }}
Replay limit: {{ item.replay_limit }}

Campaign plan from planner:
{{ nodes.plan.output }}

Rules:
- Do not patch product code.
- Do not commit or push.
- Use fake daemon unless a real-daemon confirmation is explicitly practical and safe.
- Count only real user-visible UI/UX bugs triggered by click, type, keyboard,
  resize, reconnect, stale daemon event, or response ordering.
- Reject fixture-only, environment-only, dependency-only, and tooling-only failures.
- Treat `apps/puffer-desktop/tests/fuzz/coverage-ledger.json` and all non-`.runs` framework files as
  read-only. Worker shards may only create artifacts under their own
  `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/` directory.
- Temporary replay specs and Playwright output must stay under
  `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/`.
- Leave generated `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/` artifacts for review.
- If a generated replay reveals an already-known bug, classify it as duplicate
  and keep hunting for distinct triggerable issues.
- If `bounded-replay-report.md` says `Known duplicate: yes`, or the JSON finding
  has `knownDuplicate: true`, report it as duplicate evidence unless you can
  explain a distinct root cause.
- Do not read the full `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/report.md` report unless
  a selected top-case artifact is missing. Full reports are intentionally too
  large for stable shard execution.
- Only inspect `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/top.md` and at most
  {{ item.replay_limit }} selected case IDs.
- Do not create `*.replay.spec.ts` files in the test tree during this campaign.
- Treat the owned nodes as your bug ownership boundary. Use allowed setup nodes
  only to reach the start node; if the failure belongs to a different subtree,
  record it as out-of-shard evidence instead of an accepted finding.
- Prefer bugs that exercise the listed allowed async events and required
  invariants for this shard.

Required workflow:
1. Read `apps/puffer-desktop/tests/fuzz/README.md`, `apps/puffer-desktop/tests/fuzz/agent_guide.md`, `apps/puffer-desktop/tests/fuzz/playwright_adapter.md`,
   and `apps/puffer-desktop/tests/fuzz/prompt.txt`.
2. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate`.
3. Run:
   `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed {{ item.seed }} --iterations {{ item.iterations }} --steps {{ item.steps }} --rng-seed agentflow-{{ item.name }} --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/run.json`
4. Run:
   `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/run.json --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/report.md`
5. Run:
   `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/run.json --shard {{ item.name }} --limit {{ item.replay_limit }} --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/top.md`
6. Read only `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/top.md` and use only the listed case IDs.
7. Run bounded replay with isolated specs/output:
   `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds {{ item.seed }} --shard {{ item.name }} --limit {{ item.replay_limit }} --attempts 3 --timeout 120 --rng-seed agentflow-{{ item.name }} --namespace agentflow-{{ item.name }} --fail-on-new-finding`
8. Read only `apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/bounded-replay-report.md`.
9. Shrink mentally only. Do not create extra exploratory specs unless a replay
    failure is stable and the original spec is too broad to describe.
10. Record scheduler feedback for the shard:
   `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard {{ item.name }} --input apps/puffer-desktop/tests/fuzz/.runs/agentflow-{{ item.name }}/bounded-replay-report.json`

For each accepted finding, write a detailed entry with:
- Title
- Severity estimate: P0/P1/P2
- Area/component
- Seed and replay case ID
- Trigger steps
- Expected behavior
- Actual behavior
- User impact
- Why this is product bug, not fixture/environment/tooling
- Stability: exact rerun count and result
- Relevant source files/components likely involved
- Suggested regression test target
- Error-context/screenshot/trace paths if generated

Final shard output must include:
- commands run
- replay cases tested
- accepted findings
- duplicates/rejected false positives and why
- actionable failure count versus known duplicate count from bounded replay
- remaining coverage gaps for this area
"""


with Graph(
    "puffer-uiux-agentflow-fuzz",
    description="AgentFlow campaign for Puffer UI/UX interaction fuzzing with Playwright feedback signals.",
    working_dir=REPO_ROOT,
    concurrency=2,
    fail_fast=False,
    node_defaults={
        "capture": "final",
        "retries": 0,
    },
    agent_defaults={
        "claude": {
            "model": MODEL,
            "env": CLAUDE_ENV,
            "timeout_seconds": 1500,
        }
    },
) as dag:
    preflight = shell(
        task_id="preflight",
        script=(
            "set -euo pipefail\n"
            "test -n \"${ANTHROPIC_AUTH_TOKEN:-}\"\n"
            "test \"${ANTHROPIC_BASE_URL:-}\" = \"https://api-infer.agentsey.ai\"\n"
            "rm -rf apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight apps/puffer-desktop/tests/fuzz/.runs/agentflow-campaign\n"
            + CLEAN_SELECTED_ARTIFACTS
            + "mkdir -p apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate\n"
            "echo SELECTED_AREAS=${PUFFER_AGENTFLOW_AREAS:-all}\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit ${PUFFER_AGENTFLOW_SHARD_LIMIT:-4} --namespace ${PUFFER_AGENTFLOW_NAMESPACE:-agentflow-scheduled} --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight/schedule.md --json-out apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight/schedule.json\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs frontier --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight/frontier.md\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs plan --profile core --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-preflight/plan.md\n"
            "echo PREFLIGHT_OK\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "PREFLIGHT_OK"}],
    )

    plan = claude(
        task_id="plan",
        prompt=PLANNER_PROMPT,
        tools="read_only",
        timeout_seconds=900,
    )

    fuzz_shard = fanout(
        claude(
            task_id="fuzz_shard",
            prompt=SHARD_PROMPT,
            tools="read_write",
            timeout_seconds=1500,
        ),
        SELECTED_AREAS,
    )

    aggregate_findings = shell(
        task_id="aggregate_findings",
        script=(
            "set -euo pipefail\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-agentflow-aggregate.mjs\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "AGGREGATE_OK"}],
    )

    preflight >> plan
    plan >> fuzz_shard
    fuzz_shard >> aggregate_findings


if __name__ == "__main__":
    print(dag.to_json())
