---
name: trello-webhook
description: Configure the Trello webhook connector preset with /connect so board, card, list, and comment changes reach Puffer.
---

# Trello Webhook Connector

Use this skill when the user wants Trello board, card, list, checklist, or
comment webhook events to reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect trello-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect trello-webhook trello-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9898`
- webhook URL path, defaulting to `/trello` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Trello preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Trello Configuration

Create a Trello webhook for the board, list, card, or other model the user cares
about by calling the Trello webhook API with:

- `callbackURL`, pointing at the public `puffer serve` listener and path
- `idModel`, set to the Trello model to watch
- the user's Trello API key and token

Example callback URL:

```text
https://example.com/trello
```

Trello validates the callback endpoint before saving the webhook. Puffer accepts
that validation probe and accepts future JSON deliveries that include an
`action` object plus the updated `model`. Trello deliveries may include the
`X-Trello-Webhook` signature header; Puffer uses it as a delivery hint but does
not verify the signature yet because the preset does not collect the application
secret.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Trello action
payloads can be normalized into readable Puffer messages.

Puffer groups conversations by the first stable Trello subject in the delivery,
for example:

- `trello:card:<card-id>` for card events
- `trello:list:<list-id>` for list events
- `trello:board:<board-id>` for board-level events

Messages include action type, actor, board, subject, list, URL, timestamp, and
comment text when those fields are present.
