---
name: stripe-webhook
description: Configure the Stripe webhook connector preset with /connect so payment and billing events reach Puffer.
---

# Stripe Webhook Connector

Use this skill when the user wants Stripe payment, checkout, invoice,
subscription, or dispute webhook events to reach Puffer workflows or agent
sessions.

## Setup

Run:

```text
/connect stripe-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect stripe-webhook stripe-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9696`
- webhook URL path, defaulting to `/stripe` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Stripe preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Stripe Configuration

In Stripe, create a webhook endpoint that points to the public URL for the
configured `puffer serve` listener and path, for example:

```text
https://example.com/stripe
```

Enable the event kinds the user cares about. Puffer recognizes Stripe Event
payloads with `object = "event"`, `type`, `id`, and `data.object`. Useful
events include:

- checkout session completed and expired events
- invoice payment succeeded and failed events
- customer subscription created, updated, and deleted events
- payment intent succeeded and failed events
- charge dispute events

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Stripe events can
be normalized into readable Puffer messages.

Puffer groups conversations by stable Stripe subjects when possible:

- invoice events use `stripe:<account>:invoice:<invoice-id>` when account
  metadata is present
- checkout events use `stripe:checkout.session:<session-id>`
- subscription events use `stripe:<account>:subscription:<subscription-id>`
- payment intent and charge events use their Stripe object ids

If Stripe sends an event shape Puffer does not recognize deeply, the webhook
router still groups it by object type and id when that metadata is present.
