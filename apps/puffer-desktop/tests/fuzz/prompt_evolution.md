# Puffer UI/UX Prompt Evolution Pack

This pack is the stable gold-standard checklist for small-model GUI explorer
and triage prompts. It is intentionally deterministic: the main harness may
append run-specific feedback, but workers should not rewrite these rules.

## Accepted Finding Standard

- The issue must be triggered by visible GUI interaction, keyboard input,
  pointer input, or daemon event ordering reachable from a user flow.
- The issue must block, corrupt, duplicate, or lose a core interaction result:
  send, stop, permission answer, question answer, session switch, transcript
  reload, new-agent creation, provider/model selection, Browser operation,
  terminal input, file edit/save, settings save, or MCP operation.
- The report must name the owned shard, the exact visible control or state, the
  async/reload/race condition when present, and a minimal reproduction path.
- The report must cite replay evidence: case id, attempt count, stable failure
  count, or the precise replay artifact path.
- The expected and actual behavior must be different in a product-visible way.

## False Positive Rejection Standard

- Reject fixture-only failures where the fake daemon lacks data that the real
  product does not promise.
- Reject dependency/environment failures such as missing browser binaries,
  local auth, local daemon startup, network access, or test timeout without a
  product-visible stuck state.
- Reject screenshot polish, spacing, copy, or minor visual inconsistency unless
  it blocks a required action.
- Reject out-of-shard findings unless the report explicitly marks them for
  routing to the owning shard.
- Reject claims based only on a generated sequence. A generated case is a lead;
  bounded replay or direct interaction evidence is required before promotion.
- Reject duplicate root causes already represented by the bug ledger unless
  the new path corrupts a different user intent or data object.

## Exploration Bias

- Prioritize the core loop before secondary panes: new agent, chat composer,
  turn lifecycle, stop/cancel, permission/question prompts, session switching,
  transcript reload, stale events, and draft preservation.
- Combine one owned user action with one relevant async stressor when possible:
  late success, late failure, duplicate submit, reconnect, stale event, server
  push update, or reload.
- Prefer short sequences that preserve causality over long random walks.
- Use setup nodes only to reach the shard start node; accepted findings should
  belong to owned nodes.
- Keep candidate cases materially different inside the same shard: vary the
  owned control, the async timing, or the state transition being checked.

## Validator Feedback Loop

- Explorer agents create candidate interactions only.
- Replay validates whether the interaction is stable and product-visible.
- Triage promotes only replay-backed product candidates.
- Failed validation should feed the next prompt as a concrete rejection rule:
  what looked suspicious, why it was not a product bug, and which evidence was
  missing.
- Successful validation should feed the next prompt as a target pattern:
  the triggering control, the state race, the invariant that failed, and the
  shortest stable reproducer.

## Picture-Derived Notes

- The rollout topology to emulate is analyst/planner -> independent explorers
  -> validator/triage -> feedback to planner.
- The purpose of large shard fanout is coverage diversity, not accepting more
  low-evidence findings.
- Validation failures should update the prompt/checklist before increasing
  shard count again.
