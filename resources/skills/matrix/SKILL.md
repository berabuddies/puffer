---
name: matrix
description: Configure and operate the Matrix connector through puffer serve.
allowed-tools:
  - Bash
argument-hint: "[Matrix connector task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use the `matrix-bot` connector when the user wants a Matrix account to bridge
room messages into Puffer through `puffer serve`.

Target: $target

Configuration lives in `.puffer/connectors.toml` or `~/.puffer/connectors.toml`
under `[connectors.matrix]`. The connector uses password login for a Matrix
account.

```toml
[connectors.matrix]
homeserver_url = "https://matrix.example.org"
username = "bot"
password = "..."
require_mention = true
group_key_policy = "per_user"
```

Run `puffer serve` after configuring the connector. The connector handles
inbound Matrix messages directly through the connector runtime; it is not a
workflow trigger yet and should not be used in `WorkflowCreate` connection
triggers until a typed subscriber or connector protocol stream exists.
