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

## Agent Loop

1. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs plan --profile core`.
2. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs validate` before using a changed seed.
3. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs smoke --profile core` when checking a fresh checkout or modified seed set.
4. Run `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs frontier --profile core` and pick one high-risk uncovered target.
5. Pick one seed with high priority and low recent validated coverage.
6. Run the seed with 8-20 iterations and 12-20 steps.
7. Read the report and choose a case with high async coverage.
8. Select diverse replay candidates with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input <run.json> --limit 5 --out apps/puffer-desktop/tests/fuzz/.runs/<run>/top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/<run>/top.md`.
9. Replay them through the isolated bounded loop with `node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds <seed> --limit 5 --attempts 3 --namespace <run> --fail-on-new-finding`.
10. Shrink the case.
11. Decide whether it is a product bug.
12. During fuzz-only campaigns, archive confirmed findings under
    `apps/puffer-desktop/tests/fuzz/.runs/<run>/findings.md`.
13. Do not patch product code from a fuzz-only task.
14. For a later product-fix task, add regression coverage and update or add a
    concise component spec.

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
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs frontier --profile core --out apps/puffer-desktop/tests/fuzz/.runs/manual/frontier.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs gate --out apps/puffer-desktop/tests/fuzz/.runs/manual/ready.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs run --seed chat-turn-race --iterations 12 --steps 18 --profile core --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs report --input apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs top-cases --input apps/puffer-desktop/tests/fuzz/.runs/manual/chat.json --limit 5 --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-top.json --report-out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-top.md
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz-replay-loop.mjs --seeds chat-turn-race --limit 5 --attempts 3 --namespace manual-chat --fail-on-new-finding
node apps/puffer-desktop/tests/fuzz/bin/puffer-fuzz.mjs agent-task --seed chat-turn-race --out apps/puffer-desktop/tests/fuzz/.runs/manual/chat-agent.md
```

For continuous fuzzing, prefer `--fail-on-new-finding` over
`--fail-on-finding`; it keeps known duplicate failures visible without treating
them as fresh blocker bugs.

Use secondary seeds such as `browser-tab-race` only after the core chat,
session, new-agent, turn lifecycle, reload, permission, and question paths have
acceptable coverage for the current pass.
