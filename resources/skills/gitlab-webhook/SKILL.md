---
name: gitlab-webhook
description: Configure the GitLab webhook connector preset with /connect so GitLab project or group webhook events reach Puffer.
---

# GitLab Webhook Connector

Use this skill when the user wants GitLab project or group webhook events to
reach Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect gitlab-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect gitlab-webhook gitlab-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9494`
- webhook URL path, defaulting to `/gitlab` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the GitLab preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## GitLab Configuration

In GitLab, create a project or group webhook that points to the public URL for
the configured `puffer serve` listener and path, for example:

```text
https://example.com/gitlab
```

Enable the event kinds the user cares about. Puffer recognizes GitLab webhook
headers such as `X-Gitlab-Event` and payloads with `object_kind`.

Useful events include:

- issue events
- merge request events
- comment/note events
- push events

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming GitLab events can
be normalized into readable Puffer messages.

Puffer groups conversations by stable GitLab subjects when possible:

- issues use `gitlab:<project>:issue:<iid>`
- merge requests use `gitlab:<project>:merge-request:<iid>`
- notes/comments use the parent issue, merge request, or commit thread
- pushes fall back to the pushed ref

If GitLab sends an event shape Puffer does not recognize deeply, the webhook
router still groups it by project, event kind, and delivery id when that
metadata is present.
