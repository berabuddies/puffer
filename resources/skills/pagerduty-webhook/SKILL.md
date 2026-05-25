---
name: pagerduty-webhook
description: Configure the PagerDuty webhook connector preset with /connect so incident and service events reach Puffer.
---

# PagerDuty Webhook Connector

Use this skill when the user wants PagerDuty V3 webhook subscriptions to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect pagerduty-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect pagerduty-webhook pagerduty-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9796`
- webhook URL path, defaulting to `/pagerduty` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the PagerDuty preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## PagerDuty Configuration

In PagerDuty, create a Generic Webhooks (v3) subscription and set the webhook
URL to the public `puffer serve` listener and path.

Example callback URL:

```text
https://example.com/pagerduty
```

Subscribe to incident events such as:

- `incident.triggered`
- `incident.acknowledged`
- `incident.resolved`
- `incident.reassigned`
- `incident.status_update_published`

PagerDuty V3 deliveries include a top-level `event` object with fields such as
`event_type`, `resource_type`, `occurred_at`, `agent`, and `data`. V3 requests
also identify themselves with `PagerDuty-Webhook/V3.0` in the `user-agent`
header and include headers such as `x-webhook-subscription` and
`x-pagerduty-signature`. Puffer uses these headers and event fields for routing
and readable summaries. The preset does not verify `x-pagerduty-signature` yet
because the setup flow does not collect the PagerDuty webhook signing secret.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming PagerDuty JSON
payloads can be normalized into readable Puffer messages.

Puffer groups conversations by service and the first stable PagerDuty subject in
the delivery, for example:

- `pagerduty:<service>:incident:<incident-id>`
- `pagerduty:<service>:service:<service-id>`
- `pagerduty:<service>:event:<delivery-id>`

Messages include event type, resource type, service, incident or service
subject, status, urgency, priority, URL, occurrence time, delivery id, and
message text when those fields are present.
