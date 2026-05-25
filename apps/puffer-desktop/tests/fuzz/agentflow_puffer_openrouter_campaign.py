"""Codex-planned OpenRouter small-model campaign for Puffer UI/UX fuzzing.

This campaign is intentionally separate from the full Claude/Infer campaign.
Codex acts as the main planner, and cheaper OpenAI-compatible workers
only execute bounded UI-tree shards and report trigger evidence.

Required environment:

  export OPENROUTER_API_KEY="<key>"
  # or:
  export PUFFER_OPENROUTER_API_KEY_FILE="/path/to/openrouter-key"

For local no-network orchestration smoke only:

  export PUFFER_OPENROUTER_OFFLINE_SMOKE=1

For scale-readiness plumbing checks without model or Playwright cost:

  export PUFFER_OPENROUTER_SYNTHETIC_SHARDS=1
  export PUFFER_OPENROUTER_FORCE_FALLBACK_PLAN=1

Optional controls:

  export PUFFER_OPENROUTER_PLANNER_MODEL="gpt-5.4"
  export PUFFER_OPENROUTER_PLANNER_EFFORT="high"
  export PUFFER_OPENROUTER_MODEL="inclusionai/ling-2.6-flash"
  export PUFFER_OPENROUTER_CONCURRENCY=2
  export PUFFER_OPENROUTER_SHARD_LIMIT=2
  export PUFFER_OPENROUTER_AREAS="chat-composer-send,settings-mcp"
  export PUFFER_OPENROUTER_FEEDBACK_LEDGER="apps/puffer-desktop/tests/fuzz/.runs/<run>/feedback-ledger.json"
  export PUFFER_OPENROUTER_COVERAGE_LEDGER="apps/puffer-desktop/tests/fuzz/.runs/<run>/coverage-ledger.json"

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
PLANNER_MODEL = os.environ.get("PUFFER_OPENROUTER_PLANNER_MODEL", "gpt-5.4")
PLANNER_EFFORT = os.environ.get("PUFFER_OPENROUTER_PLANNER_EFFORT", "high")
PREFLIGHT_DIR = f"apps/puffer-desktop/tests/fuzz/.runs/{NAMESPACE}-preflight"
PRESELECT_DIR = f"apps/puffer-desktop/tests/fuzz/.runs/{NAMESPACE}-scheduler-preselect"


def scheduled_areas():
    """Return scheduler-selected shards for the small-model run."""
    target_count = max(1, int(SHARD_LIMIT))
    requested = [
        item.strip()
        for item in os.environ.get("PUFFER_OPENROUTER_AREAS", "").split(",")
        if item.strip()
    ]
    evolution_json = f"{PRESELECT_DIR}/evolved-schedule.json"
    evolution_md = f"{PRESELECT_DIR}/evolved-schedule.md"
    subprocess.run(
        [
            "node",
            "apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs",
            "evolve-tree",
            "--out",
            evolution_md,
            "--json-out",
            evolution_json,
        ],
        cwd=REPO_ROOT,
        text=True,
        check=True,
        stdout=subprocess.DEVNULL,
    )
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
        "--evolution",
        evolution_json,
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
            "is_bridge": "1" if item.get("bridge") else "0",
            "bridge_left": (item.get("bridge") or {}).get("leftShard", ""),
            "bridge_right": (item.get("bridge") or {}).get("rightShard", ""),
            "bridge_witness": (item.get("bridge") or {}).get("sharedWitness", ""),
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
You are the Codex main planner for a small-model Puffer UI/UX fuzz smoke
campaign.

Repo: {REPO_ROOT}
Task file: {TASK_PATH}
Worker model family: OpenRouter small model ({MODEL})

Read only these files:
- {TASK_PATH}
- apps/puffer-desktop/tests/fuzz/README.md
- apps/puffer-desktop/tests/fuzz/agent_guide.md
- {PREFLIGHT_DIR}/prompt-evolution.md
- {PREFLIGHT_DIR}/evolved-schedule.md
- apps/puffer-desktop/tests/fuzz/playwright_adapter.md
- apps/puffer-desktop/tests/fuzz/BUGS.md

Produce a short execution plan for small-model worker shards:
- strict scope boundaries
- likely false-positive patterns
- prompt-evolution checklist items that workers must obey
- exact report format requirements
- reminder that triage writes verdict.json and verdict-gate.json, not direct BUGS edits
- reminder that workers should execute the fixed commands instead of
  improvising campaign strategy

Do not modify files.
"""


FALLBACK_PLANNER_TEXT = f"""\
# OpenRouter Small-Model UI/UX Fuzz Fallback Plan

Codex planner guidance was unavailable for this round. Continue with this
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

Triage must emit strict `verdict.json`; the deterministic citation gate writes
`verdict-gate.json`. Only the main agent may append admitted verdicts to
apps/puffer-desktop/tests/fuzz/BUGS.md.
"""


PLAN_SCRIPT = f"""\
set -euo pipefail
preflight_dir="{PREFLIGHT_DIR}"
mkdir -p "$preflight_dir"
prompt_file="$preflight_dir/planner-prompt.txt"
fallback_file="$preflight_dir/fallback-plan.md"
output_file="$preflight_dir/planner-output.md"
transcript_file="$preflight_dir/planner-transcript.log"
error_file="$preflight_dir/planner-error.log"
cat > "$prompt_file" <<'PLANNER_PROMPT_EOF'
{PLANNER_PROMPT}
PLANNER_PROMPT_EOF
cat > "$fallback_file" <<'FALLBACK_PLAN_EOF'
{FALLBACK_PLANNER_TEXT}
FALLBACK_PLAN_EOF
planner_timeout="${{PUFFER_OPENROUTER_CODEX_PLAN_TIMEOUT_SECONDS:-180}}"
if [[ "${{PUFFER_OPENROUTER_FORCE_FALLBACK_PLAN:-0}}" == "1" ]]; then
  echo "OPENROUTER_PLAN_FALLBACK forced"
  cat "$fallback_file"
elif timeout "$planner_timeout" codex exec \
  --model "${{PUFFER_OPENROUTER_PLANNER_MODEL:-gpt-5.4}}" \
  --sandbox read-only \
  -c model_reasoning_effort="${{PUFFER_OPENROUTER_PLANNER_EFFORT:-high}}" \
  --output-last-message "$output_file" \
  - \
  < "$prompt_file" \
  > "$transcript_file" 2> "$error_file" && [[ -s "$output_file" ]]; then
  cat "$output_file"
  echo
  echo "OPENROUTER_PLAN_OK codex"
else
  echo "OPENROUTER_PLAN_FALLBACK"
  if [[ -s "$error_file" ]]; then
    echo
    echo "Codex planner error excerpt:"
    tail -n 20 "$error_file"
    echo
  fi
  cat "$fallback_file"
fi
"""


SHARD_SCRIPT = """\
set -euo pipefail
out_dir="apps/puffer-desktop/tests/fuzz/.runs/{{ item.namespace }}"
preflight_dir="apps/puffer-desktop/tests/fuzz/.runs/${PUFFER_OPENROUTER_NAMESPACE:-openrouter-small}-preflight"
mkdir -p "$out_dir"
cp "$preflight_dir/prompt-evolution.md" "$out_dir/prompt-evolution.md"
cat > "$out_dir/planner.md" <<'PLANNER_EOF'
{{ nodes.plan.output }}
PLANNER_EOF
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate
if [[ "${PUFFER_OPENROUTER_SYNTHETIC_SHARDS:-0}" == "1" ]]; then
  node --input-type=module <<'SYNTHETIC_SHARD_EOF'
import { createHash } from "node:crypto";
import fs from "node:fs";

const outDir = "apps/puffer-desktop/tests/fuzz/.runs/{{ item.namespace }}";
const namespace = "{{ item.namespace }}";
const shard = "{{ item.name }}";
const seed = "{{ item.seed }}";
const value = JSON.stringify({
  caseId: `${namespace}-synthetic-0001`,
  step: 1,
  action: "synthetic-scale-check",
  phase: "fuzz",
  target: shard,
  params: {},
  coverage: []
});
const sha256 = createHash("sha256").update(value).digest("hex");
fs.writeFileSync(`${outDir}/run.json`, `${JSON.stringify({
  version: 1,
  generatedAt: new Date().toISOString(),
  seed: { id: seed },
  cases: []
}, null, 2)}\n`);
fs.writeFileSync(`${outDir}/report.md`, `# Synthetic scale shard\n\n- Namespace: ${namespace}\n- Shard: ${shard}\n`);
fs.writeFileSync(`${outDir}/top.json`, `${JSON.stringify({ version: 1, selected: [] }, null, 2)}\n`);
fs.writeFileSync(`${outDir}/top.md`, `# Synthetic top cases\n\n- None\n`);
fs.writeFileSync(`${outDir}/bounded-replay-report.json`, `${JSON.stringify({
  version: 1,
  namespace,
  shard,
  seed,
  artifactDir: outDir,
  summary: {
    total: 1,
    passed: 1,
    stableFailed: 0,
    flaky: 0,
    timeout: 0,
    knownDuplicateFailures: 0,
    knownDuplicateFindings: 0,
    newCandidateFindings: 0,
    productCandidateFindings: 0,
    nonPassingFailures: 0,
    actionableFailures: 0,
    byClassification: {}
  },
  results: [
    {
      caseId: `${namespace}-synthetic-0001`,
      status: "passed",
      classification: "synthetic-scale-check",
      coverage: [],
      attempts: [],
      stepDetails: []
    }
  ],
  findings: [],
  evidence_index: [
    {
      id: "ev-action-0001",
      type: "action",
      byte_span: [0, Buffer.byteLength(value, "utf8")],
      sha256,
      value,
      metadata: { caseId: `${namespace}-synthetic-0001`, action: "synthetic-scale-check", step: 1 }
    }
  ],
  evidence_source: `${value}\n`
}, null, 2)}\n`);
fs.writeFileSync(`${outDir}/bounded-replay-report.md`, `# Synthetic bounded replay\n\n- Namespace: ${namespace}\n- Shard: ${shard}\n- Passed: 1\n`);
SYNTHETIC_SHARD_EOF
  feedback_args=()
  if [[ -n "${PUFFER_OPENROUTER_FEEDBACK_LEDGER:-}" ]]; then
    feedback_args+=(--feedback-ledger "$PUFFER_OPENROUTER_FEEDBACK_LEDGER" --out "$PUFFER_OPENROUTER_FEEDBACK_LEDGER")
  fi
  if [[ -n "${PUFFER_OPENROUTER_COVERAGE_LEDGER:-}" ]]; then
    feedback_args+=(--ledger "$PUFFER_OPENROUTER_COVERAGE_LEDGER" --coverage-ledger-out "$PUFFER_OPENROUTER_COVERAGE_LEDGER")
  fi
  if [[ "${PUFFER_OPENROUTER_NO_COVERAGE_LEDGER:-0}" == "1" ]]; then
    feedback_args+=(--no-coverage-ledger)
  fi
  node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback "${feedback_args[@]}" --shard {{ item.name }} --input "$out_dir/bounded-replay-report.json" --namespace {{ item.namespace }}
  node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/findings.md"
  echo OPENROUTER_SHARD_OK {{ item.namespace }}
  exit 0
fi
if [[ "{{ item.is_bridge }}" == "1" ]]; then
  set +e
  node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-bridge-replay.mjs --shard {{ item.name }} --seed {{ item.seed }} --iterations {{ item.iterations }} --steps {{ item.steps }} --limit {{ item.replay_limit }} --attempts 2 --timeout 120 --rng-seed {{ item.namespace }} --namespace {{ item.namespace }} --fail-on-new-finding
  replay_status=$?
  set -e
  echo OPENROUTER_BRIDGE_REPLAY_STATUS "$replay_status"
  if [[ -s "$out_dir/bounded-replay-report.json" ]]; then
    feedback_args=()
    if [[ -n "${PUFFER_OPENROUTER_FEEDBACK_LEDGER:-}" ]]; then
      feedback_args+=(--feedback-ledger "$PUFFER_OPENROUTER_FEEDBACK_LEDGER" --out "$PUFFER_OPENROUTER_FEEDBACK_LEDGER")
    fi
    if [[ -n "${PUFFER_OPENROUTER_COVERAGE_LEDGER:-}" ]]; then
      feedback_args+=(--ledger "$PUFFER_OPENROUTER_COVERAGE_LEDGER" --coverage-ledger-out "$PUFFER_OPENROUTER_COVERAGE_LEDGER")
    fi
    if [[ "${PUFFER_OPENROUTER_NO_COVERAGE_LEDGER:-0}" == "1" ]]; then
      feedback_args+=(--no-coverage-ledger)
    fi
    node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback "${feedback_args[@]}" --shard {{ item.name }} --input "$out_dir/bounded-replay-report.json" --namespace {{ item.namespace }}
    node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/findings.md"
    if [[ -s "$out_dir/verdict-gate.json" ]] && jq -e '.disposition == "candidate"' "$out_dir/verdict-gate.json" >/dev/null; then
      node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-reviewer.mjs --verdict "$out_dir/verdict.json" --gate "$out_dir/verdict-gate.json" --replay "$out_dir/bounded-replay-report.json" --out "$out_dir/reviewer.json" || true
    fi
  else
    echo OPENROUTER_BRIDGE_REPLAY_REPORT_MISSING {{ item.namespace }}
  fi
  echo OPENROUTER_SHARD_OK {{ item.namespace }}
  exit 0
fi
explorer_args=()
if [[ "${PUFFER_OPENROUTER_OFFLINE_SMOKE:-0}" == "1" ]]; then
  explorer_args+=(--offline)
fi
node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-explorer.mjs "${explorer_args[@]}" --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --steps {{ item.steps }} --cases ${PUFFER_OPENROUTER_CASES:-1} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/run.json"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input "$out_dir/run.json" --out "$out_dir/report.md"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input "$out_dir/run.json" --shard {{ item.name }} --limit {{ item.replay_limit }} --out "$out_dir/top.json" --report-out "$out_dir/top.md"
set +e
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --input "$out_dir/run.json" --seeds {{ item.seed }} --shard {{ item.name }} --limit {{ item.replay_limit }} --attempts 2 --timeout 120 --rng-seed {{ item.namespace }} --namespace {{ item.namespace }} --fail-on-new-finding
replay_status=$?
set -e
echo OPENROUTER_REPLAY_STATUS "$replay_status"
if [[ -s "$out_dir/bounded-replay-report.json" ]]; then
  feedback_args=()
  if [[ -n "${PUFFER_OPENROUTER_FEEDBACK_LEDGER:-}" ]]; then
    feedback_args+=(--feedback-ledger "$PUFFER_OPENROUTER_FEEDBACK_LEDGER" --out "$PUFFER_OPENROUTER_FEEDBACK_LEDGER")
  fi
  if [[ -n "${PUFFER_OPENROUTER_COVERAGE_LEDGER:-}" ]]; then
    feedback_args+=(--ledger "$PUFFER_OPENROUTER_COVERAGE_LEDGER" --coverage-ledger-out "$PUFFER_OPENROUTER_COVERAGE_LEDGER")
  fi
  if [[ "${PUFFER_OPENROUTER_NO_COVERAGE_LEDGER:-0}" == "1" ]]; then
    feedback_args+=(--no-coverage-ledger)
  fi
  node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback "${feedback_args[@]}" --shard {{ item.name }} --input "$out_dir/bounded-replay-report.json" --namespace {{ item.namespace }}
  node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-triage.mjs --namespace {{ item.namespace }} --shard {{ item.name }} --seed {{ item.seed }} --model ${PUFFER_OPENROUTER_MODEL:-inclusionai/ling-2.6-flash} --out "$out_dir/findings.md"
  if [[ -s "$out_dir/verdict-gate.json" ]] && jq -e '.disposition == "candidate"' "$out_dir/verdict-gate.json" >/dev/null; then
    node apps/puffer-desktop/tests/fuzz/bin/puffer-openrouter-reviewer.mjs --verdict "$out_dir/verdict.json" --gate "$out_dir/verdict-gate.json" --replay "$out_dir/bounded-replay-report.json" --out "$out_dir/reviewer.json" || true
  fi
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
) as dag:
    preflight = shell(
        task_id="preflight",
        script=(
            "set -euo pipefail\n"
            "test -n \"${OPENROUTER_API_KEY:-}\" || test -s \"${PUFFER_OPENROUTER_API_KEY_FILE:-/dev/null}\" || test \"${PUFFER_OPENROUTER_OFFLINE_SMOKE:-0}\" = \"1\"\n"
            f"rm -rf {PREFLIGHT_DIR} apps/puffer-desktop/tests/fuzz/.runs/openrouter-campaign\n"
            + CLEAN_SELECTED_ARTIFACTS
            + f"mkdir -p {PREFLIGHT_DIR}\n"
            "node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate\n"
            f"node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-prompt --out {PREFLIGHT_DIR}/prompt-evolution.md --json-out {PREFLIGHT_DIR}/prompt-evolution.json\n"
            f"node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-tree --out {PREFLIGHT_DIR}/evolved-schedule.md --json-out {PREFLIGHT_DIR}/evolved-schedule.json\n"
            f"node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit ${{PUFFER_OPENROUTER_SHARD_LIMIT:-2}} --namespace ${{PUFFER_OPENROUTER_NAMESPACE:-openrouter-small}} --evolution {PREFLIGHT_DIR}/evolved-schedule.json --out {PREFLIGHT_DIR}/schedule.md --json-out {PREFLIGHT_DIR}/schedule.json\n"
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
