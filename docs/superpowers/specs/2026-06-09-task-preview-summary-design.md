# Task Preview Summary Design

## Goal

Monitor task history summaries should make the actionable content visible at a
glance. Source labels such as connector names, chat ids, sender names, and
transport-specific prefixes belong in metadata, not in the preview text.

## Current State

`crates/puffer-cli/src/daemon_workflows/monitor_history.rs` builds the
`summary` field returned by `task_monitor_history_list`. The current
`message_summary` function composes strings from payload scope and sender
fields, producing previews such as `Support from alice: deployment status?`.
Desktop renders that `summary` directly in the task history list and detail
header.

## Design

Move monitor history preview generation to a content-first contract:

- Build the preview from message content fields first: `message`, `snippet`,
  `text`, `body`, `subject`, `title`, `event_title`, then raw trigger text.
- Strip transport and source prefixes before classification. Examples include
  `Telegram message from ...`, `Incoming Telegram message ...`,
  `Telegram user sent ...`, `In Telegram group ...`, and
  `message from chat_id ...`.
- If the message contains a clear error, return the core error text.
- If the message is a question, return the question or shortest readable
  question intent.
- If the message is a request, return the requested action phrase.
- Otherwise return the cleaned original content, capped to 60 to 80 visible
  characters.

Keep source information in structured fields. Desktop should render source
metadata from existing payload and trigger fields, including connector,
connection, scope, sender, kind, delivery mode, lag, and history index.

## Architecture

Backend:

- Replace the source-first `message_summary` composition with small typed helper
  functions in `monitor_history.rs`.
- Keep the JSON response shape stable. `summary` remains a string, but now means
  content preview.
- Add helper tests in the same module so the preview rules are easy to evolve.

Frontend:

- Update `historyMetaLabel` in `apps/puffer-desktop/src/lib/screens/Tasks.svelte`
  to include source fields already present on the message and payload.
- Continue rendering `message.summary` in the same visual slot.

## Compatibility

No persisted workflow history format changes are required. Old history rows
will be re-rendered with the new summary rules when read through the daemon RPC.
Existing payload, raw text, and action logs stay available in the detail panel.

## Testing

- Add Rust unit tests for content-only previews across Telegram, Slack-style,
  email-style, error, question, request, and fallback cases.
- Update desktop Playwright fixtures and assertions so received-message rows
  expect a content preview and source metadata separately.
- Run focused Rust and desktop tests for the changed surfaces.
