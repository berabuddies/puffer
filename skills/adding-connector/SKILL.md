---
name: adding-connector
description: Guide for implementing or updating a Puffer connector. Use when Codex needs to add a new connector template, subscriber stream, internal connector tool, ConnectorAct action, /connect auth flow, workflow subscription support, or a puffer-connector-* Rust crate in the Puffer repo.
---

# Adding Connector

Use this skill to add connector support without bypassing Puffer's resource,
subscription, auth, and workflow contracts. Prefer typed templates and protocol
adapters over stringly command plumbing.

## Decide The Shape

Start by choosing the smallest connector shape that satisfies the request:

- **Template plus action protocol**: Add or update a `ConnectorTemplate` when
  the connector can authenticate, act, or subscribe through a subprocess
  command. Core files: `crates/puffer-subscriptions/src/catalog.rs`,
  `resources/tools/connector_template.yaml`, and
  `resources/tools/connector_register.yaml`.
- **Manifest-backed subscriber**: Use `resources/subscribers/<id>/manifest.toml`
  when a reusable process should publish events for one or more connections.
  See `crates/puffer-subscriber-runtime/src/manifest.rs`.
- **Internal tool connector**: Use `resources/internal_tools/<connector>.yaml`
  plus a runtime implementation under
  `crates/puffer-core/runtime/claude_tools/workflow/` when Puffer should expose
  lookup, auth, or read helpers directly to agents.
- **Agent proxy connector**: Update `crates/puffer-subscriptions/src/proxy.rs`
  only when external chats should proxy into Puffer agents.
- **Legacy serve connector crate**: Add a `crates/puffer-connector-*` crate and
  wire `crates/puffer-cli/src/connectors.rs` only for gateway-style bot
  connectors started by `puffer serve`.

Do not add telemetry, feedback upload, or analytics while adding a connector.

## Template Contract

For built-in templates, update `crates/puffer-subscriptions/src/catalog.rs`:

- Add a stable kebab-case `slug`.
- Fill `description`, `skill`, and `binary` with human-readable values.
- Set `requires_auth`, `can_subscribe`, and `can_proxy_agent` truthfully.
- Add a deterministic default in `suggested_connection_slug`.
- Add action definitions with JSON schemas and explicit permission metadata.
  Mark outbound side effects with `external_side_effect: true`.
- Keep output payloads compatible with `message_output_schema()` unless the
  connector really needs a custom schema.

For user-defined or command-backed connectors, use the same template shape that
`ConnectorTemplate` returns:

```json
{
  "slug": "example-login",
  "description": "Example account connector",
  "skill": "example",
  "binary": "example-connector",
  "command": ["example-connector"],
  "requires_auth": true,
  "can_subscribe": true,
  "actions": {}
}
```

## Process Protocol

Command-backed connectors must follow `crates/puffer-subscriptions/src/protocol.rs`.

Implement these operations:

- `auth-ok <connection>`: print `true`, `false`, or JSON containing `ok` or
  `success`.
- `act <connection> <action>`: read one JSON object from stdin and print either
  an empty success response, raw JSON output, or
  `{"success":true,"summary":"...","output":{...}}`.
- `subscribe`: read a `{"op":"subscribe","connection":"...","cursor":...}`
  line from stdin, emit JSONL frames to stdout, and handle later
  `{"op":"ack","cursor":"...","event_id":"..."}` commands.

Subscription stdout frames are one JSON object per line:

```json
{"type":"event","id":"stable-event-id","cursor":"durable-cursor","payload":{}}
{"type":"checkpoint","cursor":"durable-cursor"}
{"type":"health","status":"ok","detail":"optional"}
```

Use stable event ids and cursors. Include connector-specific source ids,
timestamps, sender or target ids, text, and enough context for workflow triage
or monitor tasks. Never print secrets in frames, summaries, or stderr logs.

## Auth And Connections

Authentication should end by recording a `ConnectionCreate` entry with a stable
connection slug and connector slug. Keep secrets in the user auth store,
subscriber state dir, or connector-specific private state, not in resource files
or workflow specs.

If the connector needs an interactive `/connect` flow, update the command helper
or internal tool to gather choices through `AskUserQuestion` and then register
the connection. For non-interactive auth helpers, make the helper idempotent and
safe to retry.

Add or update `resources/skills/<connector>/SKILL.md` when agents need operating
instructions for auth, lookup, reading messages, or sending actions.

## Workflow Support

Workflow triggers require a connector that can produce events:

- `can_subscribe=true`, and either `command` supports `subscribe` or
  `subscriber` points to a valid manifest.
- `workflow_tools.rs` and `subscription_create.rs` start subscribers for
  connection-backed workflows; update tests if the new connector needs a new
  support path.
- `ActionSpec::ConnectorAct` calls connector actions through the template
  action contract. Keep action inputs structured and validate them in tests.
- `ActionSpec::TriageAgent` can consume connector events, so payloads should be
  compact, source-attributed, and deterministic.

## Tests And Specs

Add focused tests with the code change:

- Template catalog tests for slug, default connection slug, actions, and
  workflow-trigger support.
- Protocol bridge tests for `auth-ok`, `act`, `subscribe`, cursor ack, and
  malformed output handling.
- Auth or `/connect` tests for connection registration and secret redaction.
- Internal tool tests for lookup/read helpers and schema compatibility.
- TUI or command tests when the connector appears in `/connect`, `/workflows`,
  `/skills`, or `/tasks`.

Update the relevant numbered spec without overwriting old specs:
`specs/puffer-subscriptions/`, `specs/puffer-resources/`,
`specs/puffer-core/`, `specs/puffer-cli/`, or the connector crate's own
`specs/<component>/` folder.

Before finishing, run targeted tests first, then broaden as risk grows. For
shared connector/runtime changes, prefer:

```bash
cargo fmt --check
cargo test -p puffer-subscriptions
cargo test -p puffer-core workflow
cargo test --workspace -- --test-threads=1
```

Honor repo constraints: public Rust functions need docstrings, new Rust files
must stay under 1000 lines, modules should stay purpose-specific, and resource
provenance must remain explicit.
