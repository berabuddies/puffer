---
name: discord
description: Configure and operate the Discord bot connector through puffer serve.
allowed-tools:
  - Bash
argument-hint: "[Discord connector task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use the `discord-bot` connector when the user wants a Discord bot to bridge
Discord messages into Puffer through `puffer serve`.

Target: $target

Configuration lives in `.puffer/connectors.toml` or `~/.puffer/connectors.toml`
under `[connectors.discord]`. The connector requires a Discord bot token.

```toml
[connectors.discord]
token = "..."
require_mention = true
group_key_policy = "per_user"
```

Run `puffer serve` after configuring the connector. The connector handles
inbound Discord messages directly through the connector runtime; it is not a
workflow trigger yet and should not be used in `WorkflowCreate` connection
triggers until a typed subscriber or connector protocol stream exists.
