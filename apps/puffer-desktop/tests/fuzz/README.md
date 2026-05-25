# Puffer Interaction Fuzz Framework

This directory contains a first-pass framework for coverage-guided UI/UX bug
hunting in the Puffer desktop app. It is intentionally independent from the
existing Playwright specs so agents can use it to choose high-value interaction
scopes before turning a confirmed failure into a deterministic regression test.

## Goal

The framework tracks product-relevant interaction coverage, not generic line
coverage. The dimensions are:

- Route coverage: which screens and modals were exercised.
- Control-action coverage: which buttons, inputs, tabs, and forms were acted on.
- State-action coverage: which product states were combined with actions.
- Async ordering coverage: which delayed, stale, duplicate, reconnect, or push
  events were combined with actions.
- Invariant coverage: which safety properties were checked after the sequence.

## Quick Start

List available coverage dimensions and seeds:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs list
```

Generate a prioritized plan:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs plan --profile core --out apps/puffer-desktop/tests/fuzz/.runs/manual/plan.md
```

Generate a scheduler-selected shard batch from the UI tree, existing coverage
ledger, and feedback from previous replay runs:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule \
  --limit 4 \
  --bridge-quota 1 \
  --namespace manual-shards \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual-shards/schedule.md \
  --json-out apps/puffer-desktop/tests/fuzz/.runs/manual-shards/schedule.json
```

The scheduler is normal-first by default: it selects normal UI-tree shards
first, then appends bridge shards up to the bridge quota. The default quota is
`floor(limit / 4)` for batches of four or more. Use `--bridge-quota` when a
campaign needs a fixed cross-feature budget.

After running a shard replay, record its feedback so later schedules can reduce
duplicates, flaky shards, and out-of-shard work:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback \
  --shard chat-composer-send \
  --input apps/puffer-desktop/tests/fuzz/.runs/manual-shards-chat-composer-send/bounded-replay-report.json
```

Maintain the main bug list from the main-agent process only:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \
  --append \
  --title "Permission approval can be submitted twice during session reload" \
  --severity P1 \
  --area chat-permission-question \
  --shard chat-permission-question \
  --evidence apps/puffer-desktop/tests/fuzz/.runs/<run>/final.md \
  --source-run <run> \
  --stability "3/3" \
  --expected "one approval request per visible intent" \
  --actual "two resolve_permission requests are sent" \
  --impact "duplicate tool execution or confusing approval state"
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \
  --append-from-verdict \
  --verdict apps/puffer-desktop/tests/fuzz/.runs/<run>/verdict.json \
  --gate apps/puffer-desktop/tests/fuzz/.runs/<run>/verdict-gate.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \
  --set-status \
  --id PUF-FUZZ-0001 \
  --status fixed \
  --note "fixed by <commit> with Playwright regression"
```

Generate a prompt-evolution pack from the gold checklist, feedback ledger,
main bug list, `/tmp/puffer_issue.md`, and supplemental picture filenames:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-prompt \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual/prompt-evolution.md \
  --json-out apps/puffer-desktop/tests/fuzz/.runs/manual/prompt-evolution.json
```

Small-model OpenRouter campaigns generate this pack during preflight and copy it
into each shard run directory. Explorer and triage prompts then read the same
gold-standard acceptance/rejection checklist, so validation feedback can reduce
false positives before increasing shard count.

Generate an evolution plan from replay feedback before selecting the next shard
batch:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-tree \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual/evolved-schedule.md \
  --json-out apps/puffer-desktop/tests/fuzz/.runs/manual/evolved-schedule.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule \
  --limit 4 \
  --namespace manual-evolved \
  --evolution apps/puffer-desktop/tests/fuzz/.runs/manual/evolved-schedule.json
```

The evolution plan is deterministic and does not mutate source shard files. It
records hot-shard split recommendations, cold-shard demotion weights, and
starvation-floor forced shards.

Generate deterministic fuzz cases for one area:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run \
  --seed chat-turn-race \
  --iterations 12 \
  --steps 18 \
  --rng-seed day2-core-chat \
  --profile core \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual-chat/chat.json
```

Generate a readable report:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report \
  --input apps/puffer-desktop/tests/fuzz/.runs/manual-chat/chat.json \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual-chat/chat.md
```

Generate a task prompt for an agent:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs agent-task \
  --seed chat-turn-race \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual-chat/agent-task.md
```

Validate the framework metadata against the manifest, adapter map, and current
fake daemon method names:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate
```

Run the local no-network selftest for evidence citation, admission, candidate,
schema, and ledger invariants:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-selftest.mjs
```

Run the full local plan verifier. It checks syntax, metadata, selftests,
bounded replay evidence, no-key deterministic triage, reviewer aggregation,
tree evolution, synthetic split/demote/starvation policy, GUIFlow smoke, two
bridge-shard replays, and a Codex-planned AgentFlow offline campaign. It writes
temporary feedback and coverage ledgers under `.runs/` instead of touching the
tracked ledgers:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-verify-plan.mjs \
  --out apps/puffer-desktop/tests/fuzz/.runs/plan-verify
```

The verifier writes both `plan-verification.json` and `plan-verification.md`.
Those reports include a `phaseCoverage` section that maps the Phase 0-9 upgrade
plan to concrete verifier steps and keeps the real OpenRouter campaign as an
explicit external gate. A phase reports `blocked_external` when local evidence
passed but a required external gate, such as real OpenRouter, is missing.

To require a real OpenRouter small-model campaign, pass
`--require-real-openrouter` with `OPENROUTER_API_KEY` already exported.

Run a one-command smoke check that validates metadata and writes a small run
plus report:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs smoke --profile core
```

Generate a Playwright replay scaffold for one generated case under an isolated
run directory:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs replay \
  --input apps/puffer-desktop/tests/fuzz/.runs/manual-chat/chat.json \
  --case-id chat-turn-race-0001 \
  --out apps/puffer-desktop/tests/fuzz/.runs/manual-chat/tests/chat-replay.spec.ts
```

The replay command derives import paths from `--out`, so scaffolds can live
under `apps/puffer-desktop/tests/fuzz/.runs/` without modifying the app test tree. The bounded loop is
the preferred way to generate and rerun replay specs:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs \
  --seeds chat-turn-race \
  --shard chat-composer-send \
  --limit 3 \
  --attempts 3 \
  --namespace manual-chat \
  --fail-on-new-finding
```

Use `--fail-on-new-finding` for continuous fuzz gates. It preserves known
duplicate evidence from `coverage-ledger.json`, but returns a non-zero status
only for actionable new replay failures.

Run the AgentFlow campaign with AgentFlow's own run database isolated under
`apps/puffer-desktop/tests/fuzz/.runs/` as well:

```sh
agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_campaign.py \
  --runs-dir apps/puffer-desktop/tests/fuzz/.runs/agentflow-local-runs \
  --output summary
```

For faster iteration, run a subset of shards by area name or seed:

```sh
PUFFER_AGENTFLOW_SHARD_LIMIT=2 \
PUFFER_AGENTFLOW_AREAS=chat-composer-send,browser-address-navigation \
agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_campaign.py \
  --runs-dir apps/puffer-desktop/tests/fuzz/.runs/agentflow-local-runs \
  --output summary
```

By default, AgentFlow asks the scheduler for a small batch of UI-tree shards
instead of using a fixed area list. `PUFFER_AGENTFLOW_SHARD_LIMIT` controls the
batch size. `PUFFER_AGENTFLOW_AREAS` can pin exact shard ids. Set
`PUFFER_AGENTFLOW_LEGACY_AREAS=1` only when comparing against the older static
area fanout.

The campaign writes a deterministic aggregate report to
`apps/puffer-desktop/tests/fuzz/.runs/agentflow-campaign/puffer_agentflow_fuzz_report.md`.
At startup it clears the selected `apps/puffer-desktop/tests/fuzz/.runs/agentflow-*` shard directories
and the aggregate output directory so reports cannot accidentally reuse stale
bounded replay results from a previous run.

Run a small Codex-planned, OpenRouter-backed campaign when testing cheaper
worker models before scaling out:

```sh
export OPENROUTER_API_KEY="<key>"
PUFFER_OPENROUTER_SHARD_LIMIT=2 \
PUFFER_OPENROUTER_CONCURRENCY=2 \
PUFFER_OPENROUTER_PLANNER_MODEL=gpt-5.4 \
PUFFER_OPENROUTER_PLANNER_EFFORT=high \
PUFFER_OPENROUTER_MODEL=inclusionai/ling-2.6-flash \
agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_openrouter_campaign.py \
  --runs-dir apps/puffer-desktop/tests/fuzz/.runs/openrouter-local-runs \
  --output summary
```

The OpenRouter campaign uses the same UI-tree scheduler and verdict/gate
handoff. Codex plans the shard boundaries and report expectations, the
OpenRouter-backed Explorer uses function tools to construct the assigned GUI
trigger sequence, the harness replays that generated case, and an
OpenRouter-backed triage step writes the shard finding report. Preflight also
writes a namespace-isolated evolution plan and passes it back into scheduling,
so parallel campaigns cannot delete each other's preflight prompt pack. Triage
now emits a strict `verdict.json`; the citation gate writes `verdict-gate.json`,
and only the main agent can append admitted verdicts to `BUGS.md` with
`bug-list --append-from-verdict`. Candidate verdicts can be reviewed with
`puffer-openrouter-reviewer.mjs`; the reviewer writes
`reviewer.json` but does not edit ledgers. It defaults to two shards and two-way
concurrency. Increase `PUFFER_OPENROUTER_SHARD_LIMIT` and
`PUFFER_OPENROUTER_CONCURRENCY` only after the small run shows acceptable
instruction-following and false-positive rates.

The aggregate report includes verdict counts, reviewer decisions, evidence
entries by type, gate failure reason counts, blocker summaries, replay totals,
and duplicate/flake-related replay counters. Use it as the campaign-level
readiness artifact instead of reading individual shard logs first.

For a local no-network orchestration smoke, set
`PUFFER_OPENROUTER_OFFLINE_SMOKE=1` and omit `OPENROUTER_API_KEY`. This uses a
deterministic offline explorer only to validate AgentFlow preflight, shard
fanout, bounded replay, deterministic no-finding triage, and aggregate output.
It is not a substitute for a real small-model campaign.

For larger plumbing-only checks, add `PUFFER_OPENROUTER_SYNTHETIC_SHARDS=1`
and `PUFFER_OPENROUTER_FORCE_FALLBACK_PLAN=1`. Synthetic shards still exercise
AgentFlow fanout, per-shard artifact directories, temporary feedback/coverage
ledgers, strict no-finding verdicts, citation gates, and aggregate reporting,
but they intentionally skip model calls and Playwright replay. This is useful
for 50-shard scale readiness; it is not bug-finding evidence.

Maintain candidate findings from the main-agent process only:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs candidate-list \
  --append \
  --verdict apps/puffer-desktop/tests/fuzz/.runs/<run>/verdict.json \
  --gate apps/puffer-desktop/tests/fuzz/.runs/<run>/verdict-gate.json \
  --evidence apps/puffer-desktop/tests/fuzz/.runs/<run>/bounded-replay-report.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs candidate-list \
  --set-status \
  --id PUF-CAND-0001 \
  --status soft-bug \
  --note "reviewer admitted"
```

For no-network reviewer plumbing tests, `puffer-openrouter-reviewer.mjs
--offline` writes a `human_queue` review when cited evidence exists. This only
validates artifact shape and ledger status handling; it is not a network-backed
reviewer decision.

Run the GUIFlow smoke benchmark through the same evidence and citation gate:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-guiflow-smoke.mjs \
  --root ~/Riema/guiflow-paper \
  --suite ~/Riema/guiflow-paper/benchmarks/smoke_suite.json \
  --out apps/puffer-desktop/tests/fuzz/.runs/guiflow-smoke
```

The smoke runner is a wiring and validation probe, not a paper result. It
expects the buggy checkout app to produce one admitted predicate-backed finding
and the fixed checkout app to produce zero admitted findings. Each case writes
`result.json`, `verdict.json`, `verdict-gate.json`, and a final-page screenshot
for manual replay and diagnosis.

## Recommended Workflow

1. Start with `schedule --limit 2` or `schedule --limit 4` for small campaigns.
2. Use the selected shard boundaries as the agent ownership boundary.
3. Pick the generated commands for one shard from the schedule output.
4. Generate 8-20 cases with a named `--rng-seed` if running manually.
5. Read the report and choose cases that include `async:late-*`,
   `async:stale-*`, `async:duplicate-submit`, or `async:reconnect`.
6. Replay chosen cases with `apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs
   --fail-on-new-finding`, which keeps specs, logs, and Playwright output under
   `apps/puffer-desktop/tests/fuzz/.runs/<namespace>/`.
7. Record replay feedback with `record-feedback --shard <id> --input <bounded-replay-report.json>`.
8. If it reproduces a product bug, ask the main agent to append the admitted
   verdict with `bug-list --append-from-verdict`; subagents should not edit
   `BUGS.md` or `BUGS_CAND.md` directly.
9. Shrink the sequence to the smallest stable
   reproducer.
10. For fuzz-only campaigns, write the finding under
   `apps/puffer-desktop/tests/fuzz/.runs/<namespace>/findings.md` and leave product fixes for a separate
   follow-up.
11. When a separate product-fix task starts, add the deterministic Playwright
   regression and concise component spec there.
12. Re-run the fuzz report and mark the covered tags as validated after the
   regression exists.
13. After a fix lands, update the corresponding `BUGS.md` entry to `fixed`
   with `bug-list --set-status`.

## Ready Metrics

For day-to-day use, treat the app as ready only when:

- P0/P1 open interaction bugs are zero.
- Core route coverage is at least 95%.
- Core control-action coverage is at least 90%.
- High-priority state-action coverage is at least 85%.
- High-priority async ordering coverage is at least 80%.
- Every fixed finding has a deterministic regression test.
- Several consecutive fuzz batches produce no new P0/P1 findings.
- Fake daemon coverage is followed by a real daemon smoke pass for auth, chat,
  Browser, terminal, and CLI/GUI connection paths.

## Files

- `manifests/puffer-ui.json`: coverage target model.
- `manifests/puffer-ui-tree.json`: UI tree model used to split agent-owned
  exploration subtrees.
- `shards/*.json`: scheduler units with start node, owned nodes, setup
  boundaries, async events, and invariants.
- `seeds/*.json`: weighted fuzz grammars for product areas.
- `adapters/playwright-actions.json`: generated-action support map.
- `coverage-ledger.json`: validated coverage and fixed finding ledger.
- `feedback-ledger.json`: scheduler feedback from replay runs.
- `BUGS.md`: main-agent-owned admitted/fixed bug ledger.
- `BUGS_CAND.md`: main-agent-owned predicate-missing candidate ledger.
- `bin/puffer-fuzz.mjs`: CLI entrypoint.
- `lib/*.mjs`: deterministic generator, coverage summarizer, and formatters.
- `playwright/pufferCoverage.ts`: reusable state, element, and trace helpers for Playwright replays.
- `agent_guide.md`: instructions for agents using this framework.
- `playwright_adapter.md`: mapping from generated actions to current Playwright
  fake daemon helpers.
- `agentflow_puffer_openrouter_campaign.py`: small-model OpenRouter campaign
  for low-cost shard smoke tests.
- `puffer-openrouter-explorer.mjs`: OpenRouter function-tool Explorer that
  turns a shard into a generated replay case.

## Current Limitation

This version generates and scores interaction cases, validates that generated
metadata is replayable, documents the Playwright mapping, and emits executable
replay scaffolds for mapped core chat/session actions and mapped Browser
actions. It does not yet execute the fuzz run JSON directly or shrink failing
traces by itself. Generated cases should still be replayed and shrunk by an
agent before becoming stable product regressions.
