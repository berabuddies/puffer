---
name: vercel-webhook
description: Configure the Vercel webhook connector preset with /connect so deployment, project, domain, flag, and firewall events reach Puffer.
---

# Vercel Webhook Connector

Use this skill when the user wants Vercel platform webhook events to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect vercel-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect vercel-webhook vercel-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9492`
- webhook URL path, defaulting to `/vercel` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Vercel preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Vercel Configuration

Create the webhook in Vercel team settings with the public `puffer serve`
listener and path, for example:

```text
https://example.com/vercel
```

Useful event selections include:

- deployment created, ready, succeeded, promoted, rollback, error, canceled,
  cleanup, and check rerun events
- project created, removed, renamed, domain, environment variable, and rolling
  release events
- domain lifecycle events
- feature flag events
- firewall attack-detected events

Vercel sends a JSON body with fields such as `id`, `type`, `createdAt`,
`region`, and `payload`. The `payload` object carries nested project,
deployment, team, user, domain, flag, links, target, plan, regions, and
environment variable fields depending on the selected event type.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Vercel JSON can be
normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Vercel identifier present:

- `vercel:deployment:<deployment-id>`
- `vercel:project:<project-id>:env:<env-var-id>`
- `vercel:project:<project-id>:domain:<domain>`
- `vercel:project:<project-id>`
- `vercel:domain:<domain>`
- `vercel:flag:<flag-key-or-name>`
- `vercel:<event-type>:<event-or-resource-id>`

Messages include event type, actor, team, deployment, project, domain, flag,
target, plan, regions, dashboard links, event creation time, and region when
those fields are present.
