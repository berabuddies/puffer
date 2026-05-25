---
name: datadog-webhook
description: Configure the Datadog webhook connector preset with /connect so monitor and event notifications reach Puffer.
---

# Datadog Webhook Connector

Use this skill when the user wants Datadog monitor or event webhooks to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect datadog-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect datadog-webhook datadog-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9594`
- webhook URL path, defaulting to `/datadog` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Datadog preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Datadog Configuration

In Datadog, create a Webhooks integration entry and set the URL to the public
`puffer serve` listener and path. Use the webhook in monitor notifications with:

```text
@webhook-<WEBHOOK_NAME>
```

Recommended custom payload:

```json
{
  "source": "datadog",
  "aggregation_key": "$AGGREG_KEY",
  "alert": {
    "cycle_key": "$ALERT_CYCLE_KEY",
    "id": "$ALERT_ID",
    "metric": "$ALERT_METRIC",
    "priority": "$ALERT_PRIORITY",
    "query": "$ALERT_QUERY",
    "scope": "$ALERT_SCOPE",
    "status": "$ALERT_STATUS",
    "title": "$ALERT_TITLE",
    "transition": "$ALERT_TRANSITION",
    "type": "$ALERT_TYPE"
  },
  "event": {
    "message": "$EVENT_MSG",
    "title": "$EVENT_TITLE",
    "type": "$EVENT_TYPE"
  },
  "hostname": "$HOSTNAME",
  "id": "$ID",
  "link": "$LINK",
  "logs_sample": "$LOGS_SAMPLE",
  "snapshot": "$SNAPSHOT",
  "tags": "$TAGS",
  "text_only_msg": "$TEXT_ONLY_MSG",
  "user": "$USER",
  "username": "$USERNAME"
}
```

Datadog supports custom headers. If a user cannot change the payload shape,
they can add a custom header named `X-Datadog-Webhook`; Puffer treats that as a
Datadog source hint when the payload includes a title, message, or alert status.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Datadog JSON can
be normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Datadog identifier present:

- `datadog:alert_cycle:<alert-cycle-key>`
- `datadog:alert:<alert-id>`
- `datadog:incident:<incident-id>`
- `datadog:event:<event-id>`
- `datadog:event:<title>`

Messages include title, transition, status, priority, host, scope, metric,
query, message body, tags, logs sample, snapshot URL, and the Datadog link when
present.
