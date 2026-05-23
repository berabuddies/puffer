"""Small AgentFlow smoke for the Puffer UI/UX fuzz campaign.

This runs the same planner + shard pattern as `agentflow_puffer_campaign.py`,
but only targets the already-proven `modal-focus-race` seed. Use it to verify
Claude/Infer auth, AgentFlow templating, Playwright replay generation, and
report handoff before launching the full campaign.
"""

import os
from pathlib import Path

from agentflow import Graph, claude, fanout, shell


REPO_ROOT = os.environ.get("PUFFER_REPO_ROOT") or str(Path(__file__).resolve().parents[4])
TASK_PATH = "apps/puffer-desktop/tests/fuzz/prompt.txt"
MODEL = "claude-opus-4-6"
CLAUDE_ENV = {
    "ANTHROPIC_API_KEY": "",
    "ANTHROPIC_MODEL": MODEL,
}

PLANNER_PROMPT = f"""\
You are planning a one-shard smoke test for the Puffer UI/UX AgentFlow fuzz campaign.

Repo: {REPO_ROOT}
Task file: {TASK_PATH}

Read:
- {TASK_PATH}
- apps/puffer-desktop/tests/fuzz/README.md
- apps/puffer-desktop/tests/fuzz/agent_guide.md
- apps/puffer-desktop/tests/fuzz/playwright_adapter.md
- apps/puffer-desktop/tests/fuzz/manifests/puffer-ui.json
- apps/puffer-desktop/tests/fuzz/coverage-ledger.json

Produce a concise plan for the modal-focus-keyboard smoke shard. Mention known
modal focus findings as a harness sanity check, and explain how to classify
duplicates versus distinct findings. Do not modify files.
"""

SHARD_PROMPT = f"""\
You are a Puffer UI/UX fuzz shard running inside an AgentFlow smoke campaign.

Repo: {REPO_ROOT}
""" + """\
Area: {{ item.name }}
Seed: {{ item.seed }}
Priority target: {{ item.priority }}
Focus: {{ item.focus }}
Iterations: {{ item.iterations }}
Steps: {{ item.steps }}

Plan:
{{ nodes.plan.output }}

Rules:
- Do not patch product code.
- Do not commit or push.
- Use fake daemon.
- Count only real user-visible UI/UX bugs.
- Treat `apps/puffer-desktop/tests/fuzz/coverage-ledger.json` and all non-`.runs` framework files as
  read-only. Do not update the ledger from an AgentFlow smoke shard.
- If `bounded-replay-report.md` says `Known duplicate: yes`, or the JSON finding
  has `knownDuplicate: true`, report it as duplicate evidence rather than a new
  bug.
- Keep temporary replay specs and Playwright output under
  `apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/`.
- Leave `apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/` artifacts for review.

Workflow:
1. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate`.
2. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed {{ item.seed }} --iterations {{ item.iterations }} --steps {{ item.steps }} --rng-seed agentflow-smoke-{{ item.name }} --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/run.json`.
3. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/run.json --out apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/report.md`.
4. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds {{ item.seed }} --limit 3 --attempts 3 --timeout 120 --rng-seed agentflow-smoke-{{ item.name }} --namespace agentflow-smoke-{{ item.name }} --fail-on-new-finding`.
5. Read `apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-{{ item.name }}/bounded-replay-report.md`.
6. Write detailed findings or duplicate classifications in your final output,
   including actionable failure count versus known duplicate count.
"""

SMOKE_AREA = [
    {
        "name": "modal-focus-keyboard-smoke",
        "seed": "modal-focus-race",
        "iterations": 12,
        "steps": 3,
        "priority": "P1",
        "focus": (
            "Smoke-test AgentFlow plus fuzz replay on New agent, Create Project, "
            "and Switch workspace modal focus/keyboard behavior"
        ),
    }
]


with Graph(
    "puffer-uiux-agentflow-smoke",
    description="Single-shard smoke test for the Puffer UI/UX AgentFlow fuzz campaign.",
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
            "timeout_seconds": 2400,
        }
    },
) as dag:
    preflight = shell(
        task_id="preflight",
        script=(
            "set -euo pipefail\n"
            "test -n \"${ANTHROPIC_AUTH_TOKEN:-}\"\n"
            "test \"${ANTHROPIC_BASE_URL:-}\" = \"https://api-infer.agentsey.ai\"\n"
            "rm -rf apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke apps/puffer-desktop/tests/fuzz/.runs/agentflow-smoke-modal-focus-keyboard-smoke\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate\n"
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
            timeout_seconds=2400,
        ),
        SMOKE_AREA,
    )

    aggregate_findings = shell(
        task_id="aggregate_findings",
        script=(
            "set -euo pipefail\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-agentflow-smoke-aggregate.mjs\n"
        ),
        timeout_seconds=120,
        success_criteria=[{"kind": "output_contains", "value": "SMOKE_AGGREGATE_OK"}],
    )

    preflight >> plan
    plan >> fuzz_shard
    fuzz_shard >> aggregate_findings


if __name__ == "__main__":
    print(dag.to_json())
