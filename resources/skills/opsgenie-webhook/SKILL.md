---
name: opsgenie-webhook
description: Configure the Opsgenie webhook connector preset with /connect so alert action callbacks reach Puffer.
---

# Opsgenie Webhook Connector

Use this skill when the user wants Opsgenie alert action callbacks to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect opsgenie-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect opsgenie-webhook opsgenie-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9592`
- webhook URL path, defaulting to `/opsgenie` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Opsgenie preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Opsgenie Configuration

In Opsgenie, add a Webhook integration and configure alert action mappings in
the "Post to Webhook URL for Opsgenie alerts" section. Set the webhook URL to
the public `puffer serve` listener and path, for example:

```text
https://example.com/opsgenie
```

Useful action mappings include:

- alert created
- alert acknowledged
- alert closed
- alert deleted
- note added
- priority, message, or description updated
- custom alert action executed

Opsgenie sends JSON with an alert action and an `alert` object. Common fields
include `action`, `alert.alertId`, `alert.alias`, `alert.tinyId`,
`alert.message`, `alert.username`, `alert.userId`, `alert.entity`,
`alert.priority`, `source`, `integrationId`, `integrationName`, and
`integrationType`. Selecting the Opsgenie options to send alert description and
details gives Puffer richer summaries for create and custom actions.

Opsgenie supports custom headers on webhook calls. If a user has a customized
payload shape, they can add `X-Opsgenie-Webhook`; Puffer treats that as an
Opsgenie source hint when the body still includes an action and readable alert
message.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Opsgenie JSON can
be normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Opsgenie identifier
present:

- `opsgenie:alert:<alert-id>`
- `opsgenie:alias:<alias>`
- `opsgenie:tiny:<tiny-id>`
- `opsgenie:integration:<integration-id>:<action>`
- `opsgenie:event:<action>`

Messages include action, integration, message, alert id, alias, tiny id,
priority, previous priority, status, entity, source, user, tags, teams,
recipients, description, details, and created or updated timestamps when those
fields are present.
