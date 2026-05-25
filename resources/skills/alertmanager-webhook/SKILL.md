---
name: alertmanager-webhook
description: Configure the Prometheus Alertmanager webhook connector preset with /connect so alert notifications reach Puffer.
---

# Alertmanager Webhook Connector

Use this skill when the user wants Prometheus Alertmanager webhook receivers to
reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect alertmanager-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect alertmanager-webhook alertmanager-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9597`
- webhook URL path, defaulting to `/alertmanager` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Alertmanager preset is served by the generic webhook connector. Start it
with:

```text
puffer serve
```

## Alertmanager Configuration

In Alertmanager, add a `webhook_config` receiver and set its `url` to the
public `puffer serve` listener and path.

Example callback URL:

```text
https://example.com/alertmanager
```

Alertmanager webhook payloads include top-level fields such as `version`,
`groupKey`, `truncatedAlerts`, `status`, `receiver`, `groupLabels`,
`commonLabels`, `commonAnnotations`, `externalURL`, and `alerts`. Each alert can
include `status`, `labels`, `annotations`, `startsAt`, `endsAt`,
`generatorURL`, and `fingerprint`. Puffer uses those fields to build stable
conversation ids and readable alert summaries.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Alertmanager JSON
can be normalized into readable Puffer messages.

Puffer groups conversations by receiver and Alertmanager group or fingerprint:

- `alertmanager:<receiver>:group:<group-key>`
- `alertmanager:<receiver>:alert:<fingerprint>`

Messages include status, receiver, alert count, group key, group labels, common
labels, common annotations, alert summaries, source URLs, truncated alert count,
and the Alertmanager external URL when present.
