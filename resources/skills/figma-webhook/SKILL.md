---
name: figma-webhook
description: Configure the Figma webhook connector preset with /connect so file, comment, library publish, and Dev Mode status events reach Puffer.
---

# Figma Webhook Connector

Use this skill when the user wants Figma webhook events to reach Puffer
workflows or agent sessions.

## Setup

Run:

```text
/connect figma-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect figma-webhook figma-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9493`
- webhook URL path, defaulting to `/figma` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Figma preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Figma Configuration

Create the webhook in Figma with the public `puffer serve` listener and path,
for example:

```text
https://example.com/figma
```

Useful event selections include:

- PING
- FILE_UPDATE
- FILE_DELETE
- FILE_VERSION_UPDATE
- LIBRARY_PUBLISH
- FILE_COMMENT
- DEV_MODE_STATUS_UPDATE

Figma sends a JSON Webhooks V2 body with fields such as `event_type`,
`webhook_id`, `file_key`, `file_name`, `timestamp`, `triggered_by`,
`comment`, `mentions`, `version_id`, `node_id`, `status`, and library asset
arrays. Puffer uses those fields to produce a readable message and choose a
stable conversation thread.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Figma JSON can be
normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Figma identifier present:

- `figma:file:<file-key>:comment:<parent-or-comment-id>`
- `figma:file:<file-key>:version:<version-id>`
- `figma:file:<file-key>:library:<webhook-or-event>`
- `figma:file:<file-key>:node:<node-id>`
- `figma:file:<file-key>:file`
- `figma:webhook:<webhook-id>`

Messages include event type, file name, file key, actor, timestamp, URL,
comment text and mentions, version label and description, library publish
counts, Dev Mode node status, and related links when those fields are present.
