---
name: github-webhook
description: Configure GitHub webhook events through the puffer serve webhook connector.
allowed-tools:
  - Bash
argument-hint: "[GitHub webhook connector task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use the `github-webhook` connector when the user wants GitHub issue, pull
request, comment, or push events to enter Puffer through `puffer serve`.

Target: $target

Set it up with:

```text
/connect github-webhook <connection>
```

The connector is a GitHub-specific preset over the HTTP webhook transport. The
deterministic `/connect` flow writes `[connectors.webhook]` in
`.puffer/connectors.toml`, usually with a `/github` path:

```toml
[connectors.webhook]
display_name = "github-webhook"
bind_address = "127.0.0.1:9292"
path = "/github"
```

Run `puffer serve` after configuring it, then create a GitHub repository
webhook that posts JSON events to the configured path. The webhook connector
normalizes GitHub payloads into agent-readable messages with the repository,
event, action, sender, subject, URL, and push commit summary when available.

This connector is setup-only in the workflow catalog for now. It is not a
native connection trigger until the webhook transport exposes a typed subscriber
or connector protocol stream.
