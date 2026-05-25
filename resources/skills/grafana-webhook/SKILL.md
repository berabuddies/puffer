---
name: grafana-webhook
description: Configure the Grafana Alerting webhook connector preset with /connect so alert notifications reach Puffer.
---

# Grafana Alerting Webhook Connector

Use this skill when the user wants Grafana Alerting webhook contact points to
reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect grafana-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect grafana-webhook grafana-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9596`
- webhook URL path, defaulting to `/grafana` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Grafana preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Grafana Configuration

In Grafana Alerting, create or update a webhook contact point and set the URL to
the public `puffer serve` listener and path.

Example callback URL:

```text
https://example.com/grafana
```

Grafana default webhook payloads include top-level fields such as `receiver`,
`status`, `orgId`, `alerts`, `groupLabels`, `commonLabels`,
`commonAnnotations`, `externalURL`, `version`, `groupKey`, `truncatedAlerts`,
`title`, `state`, and `message`. Puffer uses those fields to build stable
conversation ids and readable alert summaries.

Grafana can sign webhook requests with HMAC. Puffer currently treats
`X-Grafana-Alerting-Signature` as a shape hint only and does not verify the
signature because the setup flow does not collect the signing secret yet.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Grafana alert JSON
can be normalized into readable Puffer messages.

Puffer groups conversations by receiver and Grafana alert group or fingerprint:

- `grafana:<receiver>:group:<group-key>`
- `grafana:<receiver>:alert:<fingerprint>`

Messages include status, receiver, alert count, state, group key, common
labels, title, notification message, alert summaries, source URLs, truncated
alert count, and the Grafana external URL when present.
