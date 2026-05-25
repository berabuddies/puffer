---
name: linear-webhook
description: Configure Linear issue and project events through the puffer serve webhook connector.
allowed-tools:
  - Bash
argument-hint: "[Linear webhook connector task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use the `linear-webhook` connector when the user wants Linear issue, comment,
project, cycle, label, reaction, or SLA events to enter Puffer through
`puffer serve`.

Target: $target

Set it up with:

```text
/connect linear-webhook <connection>
```

The connector is a Linear-specific preset over the HTTP webhook transport. The
deterministic `/connect` flow writes `[connectors.webhook]` in
`.puffer/connectors.toml`, usually with a `/linear` path:

```toml
[connectors.webhook]
display_name = "linear-webhook"
bind_address = "127.0.0.1:9393"
path = "/linear"
```

Run `puffer serve` after configuring it, then create a Linear webhook that
posts JSON events to the configured path. The webhook connector recognizes
`Linear-Event` and `Linear-Delivery` headers, then normalizes body fields such
as `type`, `action`, `actor`, `data`, `updatedFrom`, and `url` into
agent-readable messages.

This connector is setup-only in the workflow catalog for now. It is not a
native connection trigger until the webhook transport exposes a typed subscriber
or connector protocol stream.
