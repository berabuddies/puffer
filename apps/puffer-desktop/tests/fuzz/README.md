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
PUFFER_AGENTFLOW_AREAS=chat-turn-lifecycle,browser-tabs-input \
agentflow run apps/puffer-desktop/tests/fuzz/agentflow_puffer_campaign.py \
  --runs-dir apps/puffer-desktop/tests/fuzz/.runs/agentflow-local-runs \
  --output summary
```

The campaign writes a deterministic aggregate report to
`apps/puffer-desktop/tests/fuzz/.runs/agentflow-campaign/puffer_agentflow_fuzz_report.md`.
At startup it clears the selected `apps/puffer-desktop/tests/fuzz/.runs/agentflow-*` shard directories
and the aggregate output directory so reports cannot accidentally reuse stale
bounded replay results from a previous run.

## Recommended Workflow

1. Start with `--profile core` unless the task explicitly targets a secondary pane.
2. Pick a seed from `apps/puffer-desktop/tests/fuzz/seeds/` that matches the target area.
3. Generate 8-20 cases with a named `--rng-seed`.
4. Read the report and choose cases that include `async:late-*`,
   `async:stale-*`, `async:duplicate-submit`, or `async:reconnect`.
5. Replay chosen cases with `apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs
   --fail-on-new-finding`, which keeps specs, logs, and Playwright output under
   `apps/puffer-desktop/tests/fuzz/.runs/<namespace>/`.
6. If it reproduces a product bug, shrink the sequence to the smallest stable
   reproducer.
7. For fuzz-only campaigns, write the finding under
   `apps/puffer-desktop/tests/fuzz/.runs/<namespace>/findings.md` and leave product fixes for a separate
   follow-up.
8. When a separate product-fix task starts, add the deterministic Playwright
   regression and concise component spec there.
9. Re-run the fuzz report and mark the covered tags as validated after the
   regression exists.

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
- `seeds/*.json`: weighted fuzz grammars for product areas.
- `adapters/playwright-actions.json`: generated-action support map.
- `coverage-ledger.json`: validated coverage and fixed finding ledger.
- `bin/puffer-fuzz.mjs`: CLI entrypoint.
- `lib/*.mjs`: deterministic generator, coverage summarizer, and formatters.
- `playwright/pufferCoverage.ts`: reusable state, element, and trace helpers for Playwright replays.
- `agent_guide.md`: instructions for agents using this framework.
- `playwright_adapter.md`: mapping from generated actions to current Playwright
  fake daemon helpers.

## Current Limitation

This version generates and scores interaction cases, validates that generated
metadata is replayable, documents the Playwright mapping, and emits executable
replay scaffolds for mapped core chat/session actions and mapped Browser
actions. It does not yet execute the fuzz run JSON directly or shrink failing
traces by itself. Generated cases should still be replayed and shrunk by an
agent before becoming stable product regressions.
