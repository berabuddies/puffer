# Monitor Pipeline (Telegram / Gmail / Calendar → Tasks → Human-approved Actions)

This is the architecture + **guardrail reference** for the monitor/triage subsystem: how an
inbound connector event (a Telegram DM, a Gmail thread, a Calendar invite) becomes a monitor
task, and how a reply/action on that task is drafted by the agent and **executed only after a
human approves it**.

If you are an agent working in this code, read **§5 Guardrails** — those are behavioral rules
the runtime enforces; violating them is a correctness/safety bug, not a style choice.

## 1. Flow at a glance

```
subscriber (resident, 1 per account)        daemon workflow runtime               bobo desktop
─────────────────────────────────────       ───────────────────────────           ──────────────
MTProto/connector update
  → delivery emit (dedup / muted / silent
     suppression; outgoing recorded)
  → envelope event ─────────────────────▶  monitor router
                                             → inbound filter chain
                                               (dedup, muted, ignore-filter,
                                                contact-filter, trigger-filter,
                                                optional classifier)
                                             → digest queue (scoped by CONVERSATION)
                                             → triage agent turn
                                                 (monitor-create prompt)
                                                 → TaskCreate  ──[pre-store GATE]──▶ monitor_tasks.json
                                                                                       │
                                                                  workflow_list ◀──────┘
                                                                  monitor_tasks[] ───────────▶ Home cards
                                             ◀── action turn (monitor-telegram-action /
                                                 monitor-gmail-action; agent DRAFTS only)
                                                 → MonitorActionDraft → pending_action
                                                                                       │
                                                              PendingActionModal ◀──────┘
                                                              (human reviews + edits + approves)
                                                                     │
                                  task_monitor_action_execute ◀──────┘ (human approval)
                                  → connector send_message / draft_reply / RSVP
```

The two long-lived processes are the **resident subscriber** (one per connected account, holds the
MTProto session) and the **daemon** (workflow runtime + RPC server). They share state on disk; see
§4. There is exactly one subscriber per account — multiple subscribers on one account kick each
other off MTProto (historical incident; do not re-introduce per-turn subscribers).

## 2. Task creation (triage → TaskCreate)

A triage agent turn runs the monitor-create prompt over one trigger or a digest batch and decides
whether to call `TaskCreate`. Before the task is written to `monitor_tasks.json`, a **server-side
pre-store gate** runs (`runtime/claude_tools/workflow/task_tools.rs`):

- **Source provenance is server-owned.** The task's source envelope must be a *current* workflow /
  digest trigger. A `monitor_envelope_id` (or `monitor_envelope_ids`) that is not in the current
  trigger batch is rejected (`untrusted_monitor_source`). The agent must NOT copy source identity
  from `conversation_context` history. The daemon stamps `monitor_connection` / `monitor_connector`
  / `monitor_envelope_id` / `chat_id` / source identity from the selected current envelope; the
  agent must not write them.
- **Read/reply gate (Telegram direct chats).** If the user already read+replied to the source
  message on Telegram, the task is skipped. The gate reads the subscriber's local
  `telegram-activity-state.json` (per-chat `read_inbox_max_id` + `agent_sent_message_ids`);
  `replied` requires a precise `reply_to == source_message_id` that is **not** an agent-originated
  send. Read-but-not-replied still creates the task, tagged `已读`. The skip returns a
  **success-shaped** result `{skipped:true, reason}` — NOT an error (so the model does not retry).
- **Duplicate suppression.** Within the same chat scope, a non-terminal task with an overlapping
  source envelope/message id, or an identical normalized subject, is treated as a duplicate.
- **Digest batches are scoped by conversation.** Events for one binding are bucketed by
  conversation (chat), not just by binding slug, so different chats never share one triage batch.

## 3. Reply / action: agent DRAFTS, human APPROVES, daemon EXECUTES

Outward actions (send a Telegram reply, create a Gmail draft, RSVP a Calendar invite) are
**human-gated**. The split:

- **Agent (action turn)** runs a scoped prompt (`monitor-telegram-action`, `monitor-gmail-action`,
  or the legacy `monitor-reply-action`) whose tool set is restricted to `MonitorActionDraft`
  (legacy: `MonitorReplyDraft`) + research tools. It PREPARES a draft and writes it to the task's
  `pending_action` (or legacy `pending_reply`). It cannot send: there is no connector send tool in
  the action turn's tool set, and `task_monitor_complete` refuses to close a human-gated task (except for the human's own `completed_via: "no_reply_needed"` decision, which is recorded with a `monitor_completion_events` audit entry, #676).
- **Human (bobo)** reviews the draft in `PendingActionModal` (or the legacy `ReplyDraftModal`),
  edits it, and approves.
- **Daemon executes** the outward effect ONLY via the `task_monitor_action_execute` RPC (legacy:
  `task_monitor_reply_send`), which is reachable only from the bobo approval UI over the
  token-authenticated WebSocket. The in-process triage/action agent cannot issue this RPC.

**Approve-what-you-see** is enforced server-side: on execute, the daemon validates the approved
pending action against the live task — `action_id` + `version` identity, and a provenance hash
(`pending_action.monitor_hash` vs the re-stamped `monitor.source_hash`), so a stale approval whose
chat/recipient/message changed is rejected. Execution is **idempotent**: an already-sent pending
action short-circuits, and a stable idempotency key (task + action + client_request_id) dedups at
the connector, so a retry or crash-after-send does not double-send.

### Typed (V2) vs legacy

Typed monitor tasks carry `schema_version=2` + a `kind` (`telegram.reply` / `gmail.reply` /
`calendar.rsvp` / `generic.review`) and route through the typed action prompts +
`MonitorActionDraft` + `PendingActionModal`. Legacy (untyped) Telegram tasks keep the
`MonitorReplyDraft` + `ReplyDraftModal` + `task_monitor_reply_send` path. The two are mutually
exclusive: `MonitorReplyDraft` rejects V2 tasks and `MonitorActionDraft` rejects untyped ones.
`generic.review` is non-executable (never carries a `pending_action`).

## 4. State on disk (under the daemon `user_config_dir`, i.e. `~/.puffer`)

- `monitor_tasks.json` — the monitor task store (source of truth for Home cards, via
  `workflow_list.monitor_tasks[]`).
- `workflow_history.json` — workflow run history (joined into the trace).
- `monitor_trace.json` — the monitor trace store (see §6). **Metadata only.**
- `telegram-accounts/<connection>/message-diagnostics.ndjson` (+ rotated `.1`) — subscriber
  delivery diagnostics.
- `telegram-accounts/<connection>/telegram-activity-state.json` — read state
  (`read_inbox_max_id`) + agent-sent message ids, maintained from the subscriber's update stream.
  **Metadata only, no message bodies** (text lives in the bounded history cache).
- `telegram-accounts/<connection>/telegram-history-cache.json` — bounded recent server history for
  triage conversation context.

## 5. Guardrails (agent-facing — the runtime enforces these)

1. **Outward actions are human-gated.** The agent only drafts (`MonitorActionDraft` /
   `MonitorReplyDraft`). A human approves in bobo; only then does the daemon send/RSVP. Never call a
   connector send tool, `MonitorReplySend`, or `TaskUpdate status:completed` to push an outward
   effect from a triage/action turn.
2. **`TaskCreate` skip is success, not failure.** A `{skipped:true, reason}` result means the gate
   intentionally suppressed the task (e.g. `handled_in_telegram`, duplicate). Do NOT retry by
   changing source metadata.
3. **Source provenance is server-owned.** Create tasks only from the *current* workflow/digest
   trigger envelope — never from `conversation_context` history. Do not write `monitor_connection`,
   `monitor_envelope_id`, `chat_id`, `source_context`, `monitor`, `pending_action`, or other
   source/delivery fields; the daemon stamps them. `TaskUpdate` must not mutate them.
4. **Conversation context is for disambiguation only.** Bounded recent history is attached to
   triage to interpret short/referential messages ("聊下？"). It is not a task source and does not
   override current source values.
5. **Same chat/contact ≠ duplicate.** A new question / topic change / separate request from the
   same sender is a new task, even when another task from that sender is still pending.
6. **Trace/diagnostics carry metadata only, never full message bodies** (text is capped to a short
   preview). Do not widen this — it is a privacy boundary.
7. **One resident subscriber per account.** Do not spawn per-turn subscribers (MTProto session
   kick-off).

## 6. Observability

- **Monitor trace** (`monitor_trace.json`): per-message trace of how far each inbound event got
  through the pipeline (subscriber received → filter stages → digest → triage entered → triage
  decision/no_task reason → task created/updated). Served to bobo via the `task_monitor_trace_list`
  RPC. **Metadata only** (sender/chat ids, stage ids, decisions, a ≤200-char text preview) — never
  full bodies. Task created/updated outcomes are recorded write-time from monitor-task store diffs;
  a digest task fans its outcome out to all contributing envelopes (`monitor_envelope_ids`).
- **Diagnostics export** (`telegram_diagnostics_export` RPC): joins `message-diagnostics.ndjson`
  (+ rotated `.1`) with `workflow_history.json` and writes a metadata-only report to `~/Downloads`
  for the user to share when debugging "why did/didn't this create a task". Privacy: the report
  redacts full message text (short snippets only).

## Related

- bobo-side flow + UI: `bobo` repo `docs/architecture/telegram-message-flow.md`.
- Bobo-facing RPC/DTO compatibility contract: see the daemon-contract doc.
- Permissions / skills / external side-effect actions: `docs/architecture/permissions-and-skills.md`.
