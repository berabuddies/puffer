---
name: azure-devops-webhook
description: Configure the Azure DevOps webhook connector preset with /connect so code, pull request, and work item service hook events reach Puffer.
---

# Azure DevOps Webhook Connector

Use this skill when the user wants Azure DevOps Service Hooks to reach Puffer
workflows or agent sessions.

## Setup

Run:

```text
/connect azure-devops-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect azure-devops-webhook azure-devops-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9493`
- webhook URL path, defaulting to `/azure-devops` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the Azure DevOps preset is served by the generic webhook connector. Start it
with:

```text
puffer serve
```

## Azure DevOps Configuration

In Azure DevOps, open Project settings, select Service hooks, create a
subscription, choose Web Hooks, and set the URL to the public `puffer serve`
listener and path, for example:

```text
https://example.com/azure-devops
```

Useful event selections include:

- Code pushed
- Pull request created
- Pull request updated
- Pull request merge attempted
- Pull request commented on
- Work item created
- Work item updated
- Work item commented on

Azure DevOps sends a JSON Service Hooks body with common fields such as
`eventType`, `publisherId`, `message`, `detailedMessage`, `resource`, and
`resourceContainers`. Puffer uses those fields to produce a readable message
and choose a stable conversation thread.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming Azure DevOps JSON
can be normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable Azure DevOps identifier
present:

- `azure-devops:<project>:pull-request:<id>`
- `azure-devops:<project>:work-item:<id>`
- `azure-devops:<project>:push:<repository>:<branch-or-reference>`
- `azure-devops:<project>:<event>:<delivery-or-event>`

Messages include event type, project, actor, repository, pull request title,
branch names, work item type, state, URL, detailed message text, and a short
commit summary when those fields are present.
