# Agent Guide: Coverage-Guided UI/UX Bug Hunting

Use this guide when an agent is asked to find Puffer desktop interaction bugs.

## Operating Rules

- Use generated cases as a guide, not as proof.
- A finding counts only if it is reproducible by user interaction or daemon
  response ordering.
- If a replay report marks `Known duplicate: yes` or `knownDuplicate: true`,
  keep the evidence but classify it as a duplicate of the known product
  candidate unless the trigger path demonstrates a distinct root cause.
- Do not count fixture-only, environment-only, or cosmetic-only issues.
- Prefer fake daemon for race construction.
- Re-check high-value failures with real daemon when the path exists there.
- Convert confirmed failures into deterministic Playwright specs.
- Do not edit `apps/puffer-desktop/tests/fuzz/BUGS.md` from a subagent. Report
  a finding block in your shard report; the main agent appends or updates the
  central bug list.

## Agent Loop

1. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit 4 --out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.md --json-out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.json`.
2. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate` before using a changed seed.
3. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs smoke --profile core` when checking a fresh checkout or modified seed set.
4. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs frontier --profile core` and pick one high-risk uncovered target.
5. Generate prompt guidance with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs evolve-prompt --out apps/puffer-desktop/tests/fuzz/.runs/<run>/prompt-evolution.md`.
6. Pick one scheduled shard with high score and respect its owned-node boundary.
7. Run the seed with 8-20 iterations and 12-20 steps.
8. Read the report and choose a case with high async coverage.
9. Select diverse replay candidates with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input <run.json> --shard <shard-id> --limit 5 --out apps/puffer-desktop/tests/fuzz/.runs/<run>/top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/<run>/top.md`.
10. Replay them through the isolated bounded loop with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds <seed> --shard <shard-id> --limit 5 --attempts 3 --namespace <run> --fail-on-new-finding`.
11. Record feedback with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard <shard-id> --input apps/puffer-desktop/tests/fuzz/.runs/<run>/bounded-replay-report.json`.
12. Shrink the case.
13. Decide whether it is a product bug using the prompt-evolution acceptance and false-positive checklist.
14. During fuzz-only campaigns, archive confirmed findings under
    `apps/puffer-desktop/tests/fuzz/.runs/<run>/findings.md`.
15. For each accepted finding, include a `BUG_LIST_APPEND` block in the shard
    report so the main agent can append it to `BUGS.md`.
16. Do not patch product code from a fuzz-only task.
17. For a later product-fix task, add regression coverage and update or add a
    concise component spec.

## Shard Ownership

Each scheduler shard has a `startNode`, `ownedNodes`, `allowedSetupNodes`,
`allowedAsyncEvents`, and required `invariants`. Use setup nodes only as a path
to the start node. Accept findings only when the blocked interaction belongs to
the owned nodes. If the trigger exposes a real issue in a different subtree,
record it as out-of-shard evidence so the scheduler can route it to the owner.

## Product Bug Threshold

Accept the issue if it blocks or corrupts:

- launch, onboarding, auth, provider/model selection, or new-agent creation
- prompt send, stop, permission answer, question answer, or transcript display
- Browser open, navigation, input, tab close, frame rendering, or chat Browser
  tool usage
- terminal input, close, focus, or pty routing
- file draft preservation, save, reload, or stale restore
- settings save, credential import, MCP add/update/remove/test, or permissions
  save

Reject the issue if it is only:

- fake daemon fixture mismatch
- missing local dependency
- screenshot-only polish that does not block interaction
- expected disabled state with clear recovery

## Bug List Handoff

The central ledger is `apps/puffer-desktop/tests/fuzz/BUGS.md`. It is owned by
the main agent to avoid concurrent edits and context drift. Subagents should
append this block to their final report for every accepted bug:

```text
BUG_LIST_APPEND
title: <short user-visible bug title>
status: pending
severity: P0|P1|P2
area: <component or flow>
shard: <shard id>
source-run: <run namespace>
evidence: apps/puffer-desktop/tests/fuzz/.runs/<run>/final.md
stability: <for example 3/3>
expected: <expected behavior>
actual: <actual behavior>
impact: <user impact>
repro: <minimal steps>
notes: <duplicate/out-of-shard/source pointers if relevant>
END_BUG_LIST_APPEND
```

The main agent should append it with:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \
  --append \
  --title "<title>" \
  --status pending \
  --severity P1 \
  --area "<area>" \
  --shard "<shard>" \
  --source-run "<run>" \
  --evidence "<evidence>" \
  --stability "<stability>" \
  --expected "<expected>" \
  --actual "<actual>" \
  --impact "<impact>" \
  --repro "<repro>" \
  --notes "<notes>"
```

When a product fix lands, update the entry with:

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs bug-list \
  --set-status \
  --id PUF-FUZZ-0001 \
  --status fixed \
  --note "fixed by <commit> with <test file>"
```

## How To Read Generated Cases

Each generated case has:

- `caseId`: stable id for the generated sequence.
- `rngSeed`: deterministic seed to reproduce generation.
- `steps`: setup, fuzz actions, and invariant assertions.
- `coverage`: tags for the route/control/state/async/invariant matrix.

Action kinds:

- `ui`: click, focus, selection, or visible control operation.
- `keyboard`: typing or key dispatch.
- `daemon`: fake daemon delay/failure/reconnect setup.
- `daemon-event`: fake daemon push event.
- `assertion`: invariant that must hold after replay.

## Shrinking Rule

When a case fails, remove steps until the failure disappears, then restore the
last removed step. The final regression should normally be 4-10 steps:

1. Open the relevant screen.
2. Put the UI in the target state.
3. Trigger the stale/late/duplicate/reconnect condition.
4. Perform the user action that exposes the bug.
5. Assert the blocked or corrupted behavior.

## Useful Commands

```sh
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs smoke --profile core
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs schedule --limit 4 --out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.md --json-out apps/puffer-desktop/tests/fuzz/.runs/manual/schedule.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs frontier --profile core --out apps/puffer-desktop/tests/fuzz/.runs/manual/frontier.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs gate --out apps/puffer-desktop/tests/fuzz/.runs/manual/ready.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed chat-turn-race --iterations 12 --steps 18 --profile core --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json --shard chat-composer-send --limit 5 --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-top.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds chat-turn-race --shard chat-composer-send --limit 5 --attempts 3 --namespace manual-chat --fail-on-new-finding
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs record-feedback --shard chat-composer-send --input apps/puffer-desktop/tests/fuzz/.runs/manual-chat/bounded-replay-report.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs agent-task --seed chat-turn-race --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-agent.md
```

For continuous fuzzing, prefer `--fail-on-new-finding` over
`--fail-on-finding`; it keeps known duplicate failures visible without treating
them as fresh blocker bugs.

Use secondary seeds such as `browser-tab-race` only after the core chat,
session, new-agent, turn lifecycle, reload, permission, and question paths have
acceptable coverage for the current pass.
