---
name: bitbucket-webhook
description: Configure the Bitbucket webhook connector preset with /connect so repository, push, and pull request events reach Puffer.
---

# Bitbucket Webhook Connector

Use this skill when the user wants Bitbucket Cloud repository callbacks to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect bitbucket-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect bitbucket-webhook bitbucket-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9493`
- webhook URL path, defaulting to `/bitbucket` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Bitbucket preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Bitbucket Configuration

In Bitbucket Cloud, add a repository or workspace webhook and set the URL to the
public `puffer serve` listener and path, for example:

```text
https://example.com/bitbucket
```

Useful event selections include:

- repository push
- pull request created, updated, approved, fulfilled, or rejected
- pull request comment created or updated
- issue created, updated, or commented

Bitbucket sends the event key in the `X-Event-Key` header and includes a JSON
body with common fields such as `actor`, `repository`, `push`, `pullrequest`,
`issue`, and `comment`. Puffer uses those fields to produce a readable message
and choose a stable conversation thread.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Bitbucket JSON can
be normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Bitbucket identifier
present:

- `bitbucket:<workspace/repo>:pull-request:<id>`
- `bitbucket:<workspace/repo>:issue:<id>`
- `bitbucket:<workspace/repo>:push:<branch-or-reference>`
- `bitbucket:<workspace/repo>:<event>:<request-or-event>`

Messages include event key, repository, actor, pull request or issue title,
branch names, state, URL, comment body, and a short commit summary when those
fields are present.
