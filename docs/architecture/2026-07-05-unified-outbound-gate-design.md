# Unified Outbound Action Approval Gate (plan-05 / agentenv#767) Design

Date: 2026-07-05
Branch: `plan-05/unified-outbound-gate`
Related issues: agentenv/monorepo#767 (parent), #728 / #561 / #634 (children)
Constraints: no backward compatibility; optimize for long-term maintainability and stability; avoid over-engineering.

## 1. Problem

"Send a message outward" currently passes through 4 non-shared policy checks and 3 parallel draft state machines:

| Layer | Location | Defect |
|---|---|---|
| Tool layer, ConnectorAct | `connector_tools.rs:646` | `send_like_action_slug` heuristic is fragile; looks up user templates that can override builtins with weaker permissions (#728-V1/V2) |
| Tool layer, MonitorReplySend | `task_tools.rs:473/2677` | Non-human-gated tasks send directly without review; the predicate sniffs metadata shapes (#728-V3) |
| Daemon layer | `daemon.rs:5337` | A second, drifting implementation of `monitor_task_is_human_gated` |
| Approval RPCs | `monitor_reply_send` / `monitor_action_execute` / `connector_action_execute` | Three independent state machines; **no cancel RPC exists** — cancellation is a client-local gesture only (#561) |

Three draft stores: `pending_reply` and `pending_action` (embedded in task metadata) plus `outbound_action_drafts.json` (standalone file).
Monitor action turns are hard-locked by `allowed_tools` plus explicit prompt prohibitions, so even an explicit user instruction cannot start a reviewed send to another recipient (#634).

## 2. Core Invariant

**Exactly one function in the codebase can execute an LLM-initiated external send: `OutboundStore::execute_approved()`, and it only accepts actions in the `approved` state.**

The approval card intercepts model volition, not user volition:

- LLM-initiated (agent session / task session / background task) → always lands as a draft first, sent only after human approval. No ungated branch.
- Rule automation (subscriptions ActionDispatcher, fired by user-configured monitor rules) → exempt from the gate (configuring the rule is standing approval), but audited.
- Human-approved → executed.

## 3. Architecture

Placement: two new modules in `puffer-subscriptions` (no new crate):

- `outbound_gate.rs` — pure decision function
- `outbound_store.rs` — single store + state machine

Dependency direction already works: both `puffer-core` (tool layer) and `puffer-cli` (daemon RPCs) depend on subscriptions, and the catalog/permission source of truth already lives there.

```
Model volition                          Human volition
──────────────                          ──────────────
ConnectorActionDraft (sole draft tool)   approve/cancel RPC (sole verdict entry)
        │                                    │
        ▼                                    ▼
   ┌─────────────────────────────────────────────┐
   │ OutboundStore (~/.puffer/outbound_actions.json)│
   │ draft_ready ──approve──▶ sending ──▶ sent    │
   │     │  │                    │                │
   │  cancel TTL expiry       send failure        │
   │     ▼  ▼                    ▼                │
   │ cancelled expired    failed / uncertain      │
   └─────────────────────────────────────────────┘
        ▲
   OutboundGate::evaluate() — pure function
```

## 4. Data Model

Single action record schema (replaces all three existing draft shapes):

```
id, version,
connector_slug, connection_slug, action, input,
recipient_stable_id, recipient_source: "stamped" | "model",
message, content_hash,
origin { session_id, turn_id, task_id? },
status,           # draft_ready | sending | sent | cancelled | expired | failed | uncertain
created_at, expires_at,   # default 24h TTL
approved_message, approved_by, approved_at,
client_request_id, send_attempt_id,
receipt, error,
events[]          # lifecycle events (same shape as today's monitor_reply_events)
```

Key points:

- **Monitor tasks no longer embed drafts**: task metadata stores only an `outbound_action_id` reference; the `pending_reply` / `pending_action` embedded shapes and their state-machine code are deleted entirely.
- **Recipient stamping**: when `ConnectorActionDraft` carries a `task_id` — ① it must match the current turn's `monitor_reply_scope` (existing scope binding, prevents writing arbitrary tasks); ② the server resolves the recipient from the task's source_context and **overrides** the model input (`recipient_source: stamped`; the model can never choose the recipient of a task reply). Without `task_id` (plain session, or an explicit instruction to message a third party), the recipient comes from model input (`recipient_source: model`); the approval card renders the distinction for human verification.
- **`cancelled` / `expired` are terminal**: cannot be superseded, cannot be approved. "Cancel, then ask to send again" = a brand-new action (new id, new version).
- Concurrency: existing per-id lock pattern (see `DRAFT_LOCKS`), atomic file writes.

## 5. Gate Decision

```rust
enum SendOrigin {
    LlmInitiated { session_id, turn_id, task_id: Option<String> },
    RuleAutomation { rule_id },   // includes workflow-engine forward/send nodes (user config = standing approval)
}
fn evaluate(origin, connector_slug, action_slug, catalog) -> GateDecision
// GateDecision: Allowed { reason } | RequiresDraft
```

The gate evaluates only at initiation time (ConnectorAct / draft creation / rule dispatch). The execute RPC runs after human approval and does not pass the gate again; disconnected accounts, unknown actions and the like are validation errors, not gate decisions — there is no `Blocked` variant and no `HumanApproved` origin.

Rules (a net simplification, no new branches):

1. `LlmInitiated` + external send action → `RequiresDraft`, no exceptions. The two send-path `monitor_task_is_human_gated` predicates are deleted. **Deletion boundary**: the `completion_policy` metadata and its "task completion requires human confirmation" triage semantics stay (the TaskUpdate / mark-done flow still uses them); this design removes only its send-gating use.
2. `RuleAutomation` → `Allowed` + audit.
3. **What counts as an external send is decided solely by the builtin catalog**: the `send_like_action_slug` heuristic is deleted; any catalog action with `external_side_effect: true` is `RequiresDraft`. Light actions exempted by design (e.g. `react`) must be explicitly whitelisted in the catalog as `category: external_reaction` — never guessed from slugs.
4. **Template hardening (closes #728-V1)**: `ConnectorCatalogStore::upsert` validates that a user template overriding a builtin slug must not weaken any action's `category` / `external_side_effect` relative to the builtin action of the same name; when the gate reads the catalog, builtin permissions act as the floor in a merge.

## 6. RPC Surface (daemon)

Three send RPC families collapse into three unified methods; the old RPCs are deleted outright:

| New RPC | Replaces |
|---|---|
| `outbound_action_execute {action_id, version, approved_message, client_request_id}` | `monitor_reply_send` + `monitor_action_execute` + `connector_action_execute` |
| `outbound_action_cancel {action_id, version, reason?}` | (did not exist — the root fix for #561) |
| `outbound_action_status {action_id, version}` | `connector_action_draft_status` |

- `execute` keeps today's anti-duplication semantics: version check, stale `sending` → `uncertain` → `duplicate_risk_ack_required`. Note: the legacy `created_by`/forged-provenance check is intentionally obsoleted — the unified store has a single creation path (`OutboundStore::create_draft`), so every record is provenance-stamped by construction and there is no second writer to forge against; the version + `client_request_id` idempotency keys replace it.
- **Executor abstraction**: `execute` dispatches per action — telegram-style connector sends go through `installed_connector_action_executor`; gmail.reply goes through the existing browser-workflow executor. Gmail's multi-stage process (create draft / open thread / …) no longer mints extra statuses; it collapses into the `sending` state with stages recorded in `events[]`, terminating in `sent`/`failed`.
- **Task write-back**: when `origin.task_id` is present, `sent` writes back to the task (receipt + mark completed); `cancelled`/`expired` clears the task's `outbound_action_id` reference. This logic lives once in the unified `outbound_action.rs`, replacing the per-workflow copies.
- **Snapshot surface (BOBO's data source)**: the task snapshot (`handle_workflow_list`/task_snapshot) joins the action record by `outbound_action_id` and embeds it in the task snapshot; BOBO renders approval cards from the snapshot as before, while desktop renders the tool card from tool output plus `outbound_action_status` polling.
- Lazy TTL: at execute time, `now > expires_at` → reject and mark `expired`. No background sweeper.
- BOBO and desktop adapt to the new RPCs together (⚠️ requires BOBO changes; parent issue labeled in-review). The desktop approval card (ToolCard connector-draft) gains a Cancel button calling `outbound_action_cancel`.

## 7. Tool Layer

- `ConnectorActionDraft` becomes the only draft tool, extended with a `task_id` parameter (monitor scenarios write the task reference back).
- Delete `MonitorReplyDraft`, `MonitorActionDraft`, `MonitorReplySend` and their dispatch branches.
- `ConnectorAct` remains for non-send actions (read_history etc.); send actions get a guiding error: "use ConnectorActionDraft".
- **Monitor action turns (#634)**: `monitor-telegram-action.yaml` / `monitor-reply-action.yaml` change `allowed_tools` to `ConnectorActionDraft + WebSearch + WebFetch + AskUserQuestion`; the prompt wording becomes "the task reply's recipient is fixed to the task source; only under an explicit user instruction may you draft to another recipient, which goes through the same human review".

## 8. Audit

- Every gate decision appends one line to `~/.puffer/outbound_audit.ndjson`:
  `{at_ms, origin, connector, action, decision: allowed_rule|draft_required|approved_send|cancelled|expired, action_id?, rule_id?}`
- The action record's `events[]` keeps lifecycle events (draft_created / cancelled / send_started / sent / send_failed …).
- Division of duty: the NDJSON answers "is the gate consistent overall" (regression verification, greppable); `events[]` answers "what happened to this action" (approval card, debugging).
- Audit write failures never block sends (best-effort + stderr warning).

## 9. Deletion List (a primary payoff of this design)

- Daemon workflows: `monitor_reply_send.rs`, `monitor_action_execute.rs`, `connector_action_execute.rs` → merged into one `outbound_action.rs`
- The two send-path `monitor_task_is_human_gated` + `monitor_task_has_telegram_delivery_target`
- Tools: `MonitorReplySend` / `MonitorReplyDraft` / `MonitorActionDraft` and their dispatch branches
- The `send_like_action_slug` heuristic
- All reads/writes of `pending_reply` / `pending_action` in task metadata
- Legacy on-disk draft data: not migrated, simply ignored (unread fields = naturally void, no send risk)

**Kept (do not over-delete)**: the daemon's `resolve_monitor_reply_turn_scope` still needs a minimal "task has a replyable target" check to decide whether to grant the action-turn scope — keep `monitor_task_has_delivery_target` (the daemon.rs version) solely for scope resolution, converging on a single copy.

## 10. Error Handling

- Send failure: `sending → failed`, re-approvable for retry.
- Stale `sending` left by a crashed process: probed and marked `uncertain`; requires `duplicate_risk_ack` before retry (existing semantics).
- Unknown action / version mismatch / terminal action: both execute and cancel fail loudly; no silent fallbacks.

## 11. Test Matrix (maps to the #767 acceptance table)

1. Agent session sends TG directly → draft card must appear; sends only after approve (tool-layer unit tests + daemon RPC integration tests).
2. Approve/execute after cancel → rejected, terminal state irreversible; superseding a cancelled action → rejected.
3. Cancel then explicitly ask to send again → new action id, new human review.
4. Task session with explicit instruction to message a third party → draft card (`recipient_source: model`), sent after approval.
5. Model decides to send without explicit instruction → gate `RequiresDraft`, never a silent send.
6. User template weakening builtin permissions → upsert rejected.
7. TTL expiry → execute rejects and marks `expired`.
8. Migrate the existing anti-duplication test family in `monitor_reply_send.rs` (forged provenance / stale sending / version mismatch) to the unified RPC.
9. Drafts with `task_id`: scope mismatch → rejected; recipient is server-stamped, overriding model input.
10. After `sent`, task write-back of receipt + completed; task snapshot embeds the joined action record (BOBO rendering surface).

Performance note: the approval path is low-frequency; no extra performance work. File locking + atomic writes match the status quo.

## 12. Explicitly Not Doing (over-engineering guards)

- Gating rule automations / per-message confirmation (Q1 decision: exempt + audit).
- An intent-detection mechanism for "explicit instruction" (#634 is solved by tool unlock + prompt wording).
- Implicit draft invalidation (new turn / turn stop) — turn-cancel propagation belongs to plan-06.
- Background TTL sweeper (lazy expiry suffices).
- A new standalone crate.
- Legacy data migration.

## Out of Scope (matches #767)

- Turn-cancel propagation and background-task reaping (plan-06).
- Approval dialog UX and ACL determinism (plan-09).
