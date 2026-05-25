---
name: asana-webhook
description: Configure the Asana webhook connector preset with /connect so task, project, and story changes reach Puffer.
---

# Asana Webhook Connector

Use this skill when the user wants Asana task, project, story, goal, or
portfolio webhook events to reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect asana-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect asana-webhook asana-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9797`
- webhook URL path, defaulting to `/asana` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Asana preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Asana Configuration

Create an Asana webhook for the resource the user cares about and point its
target URL at the public `puffer serve` listener and path, for example:

```text
https://example.com/asana
```

Asana performs an initial webhook handshake by sending `X-Hook-Secret`; Puffer
echoes that header. Future deliveries contain `X-Hook-Signature` and a JSON
body with `events`. Empty `events` arrays are heartbeats and are acknowledged
without starting an agent turn.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Asana event arrays
can be normalized into readable Puffer messages.

Puffer groups conversations by the first stable Asana resource in the delivery,
for example:

- `asana:task:<task-gid>` for task events
- `asana:project:<project-gid>` for project events
- `asana:story:<story-gid>` for story/comment events

Multiple Asana events in one delivery are summarized in a single message, with
resource, action, parent, actor, timestamp, and change-field details when those
fields are present.
