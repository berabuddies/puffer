---
name: sentry-webhook
description: Configure the Sentry webhook connector preset with /connect so issue, event, and alert deliveries reach Puffer.
---

# Sentry Webhook Connector

Use this skill when the user wants Sentry issue, event, or alert webhooks to
reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect sentry-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect sentry-webhook sentry-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9798`
- webhook URL path, defaulting to `/sentry` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Sentry preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Sentry Configuration

Sentry can deliver webhooks through Integration Platform webhooks or project
service hooks. Use an HTTPS URL pointing at the public `puffer serve` listener
and path.

Example callback URL:

```text
https://example.com/sentry
```

For project service hooks, subscribe to events such as:

- `event.alert`
- `event.created`

For Integration Platform webhooks, enable the resources and alert rule actions
the user wants Puffer to receive. Sentry integration webhooks include headers
such as `Sentry-Hook-Resource`, `Sentry-Hook-Timestamp`, and
`Sentry-Hook-Signature`. Puffer uses resource and timestamp headers for routing
and readable message summaries. The preset does not verify
`Sentry-Hook-Signature` yet because the setup flow does not collect a Sentry
integration secret.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Sentry JSON
payloads can be normalized into readable Puffer messages.

Puffer groups conversations by project and the first stable Sentry subject in
the delivery, for example:

- `sentry:<project>:issue:<issue-id>`
- `sentry:<project>:event:<event-id>`
- `sentry:<project>:alert:<alert-id>`

Messages include resource, action, project, issue or event subject, alert rule,
status, level, culprit, environment, URL, event id, delivery id, and message text
when those fields are present.
