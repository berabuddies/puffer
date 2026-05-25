---
name: jira-webhook
description: Configure the Jira webhook connector preset with /connect so Jira issue and comment events reach Puffer.
---

# Jira Webhook Connector

Use this skill when the user wants Jira issue or comment webhook events to
reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect jira-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect jira-webhook jira-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9595`
- webhook URL path, defaulting to `/jira` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Jira preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## Jira Configuration

In Jira, create an admin webhook or REST-registered webhook that points to the
public URL for the configured `puffer serve` listener and path, for example:

```text
https://example.com/jira
```

Enable the event kinds the user cares about. Puffer recognizes Jira payloads
with `webhookEvent`, `issue_event_type_name`, `issue`, `comment`, and `project`
fields. Useful events include:

- issue created, updated, and deleted events
- comment created, updated, and deleted events
- project events when they are enabled

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Jira events can be
normalized into readable Puffer messages.

Puffer groups conversations by stable Jira subjects when possible:

- issue events use `jira:<project>:issue:<issue-key>`
- comment events use the parent issue thread
- project events use `jira:<project>:project:<project-key>`

If Jira sends an event shape Puffer does not recognize deeply, the webhook
router still groups it by project, event kind, and delivery id when that
metadata is present.
