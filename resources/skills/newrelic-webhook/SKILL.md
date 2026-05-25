---
name: newrelic-webhook
description: Configure the New Relic webhook connector preset with /connect so issue and alert notifications reach Puffer.
---

# New Relic Webhook Connector

Use this skill when the user wants New Relic issue or alert webhooks to reach
Puffer workflows or agent sessions.

## Setup

Run:

```text
/connect newrelic-webhook <connection-name>
```

If no connection name is supplied, ask for one. The deterministic default is:

```text
/connect newrelic-webhook newrelic-webhook
```

The setup flow asks for:

- bind address, such as `127.0.0.1:9593`
- webhook URL path, defaulting to `/newrelic` when left blank

The command writes `[connectors.webhook]` in `.puffer/connectors.toml` because
the New Relic preset is served by the generic webhook connector. Start it with:

```text
puffer serve
```

## New Relic Configuration

In New Relic, create a notification destination using the Webhook destination
type. Set the webhook URL to the public `puffer serve` listener and path, use
`POST`, and use `JSON` for the payload. Then attach that destination to a
workflow so issues are sent to Puffer.

Recommended custom payload:

```json
{
  "source": "newrelic",
  "issueId": "{{ issueId }}",
  "issueTitle": "{{ issueTitle }}",
  "priority": "{{ priority }}",
  "state": "{{ state }}",
  "stateText": "{{ stateText }}",
  "triggerEvent": "{{ triggerEvent }}",
  "issuePageUrl": "{{ issuePageUrl }}",
  "incidentIds": {{ json accumulations.incidentIds }},
  "entitiesData": {{ json entitiesData }},
  "accumulations": {
    "conditionDescription": {{ json accumulations.conditionDescription }},
    "conditionName": {{ json accumulations.conditionName }},
    "policyName": {{ json accumulations.policyName }},
    "runbookUrl": {{ json accumulations.runbookUrl }}
  },
  "nrAccountId": "{{ nrAccountId }}",
  "workflowName": "{{ workflowName }}"
}
```

New Relic also supports custom headers on webhook destinations. If a user
cannot change the payload shape, they can add a custom header named
`X-NewRelic-Webhook`; Puffer treats that as a New Relic source hint when the
payload includes a title, details, or state.

## Runtime Behavior

This preset is setup-only in the workflow connector catalog. It does not expose
a connector protocol subscriber, workflow trigger runtime, or outbound action
yet. It prepares the serve-mode webhook endpoint so incoming New Relic JSON can
be normalized into readable Puffer messages.

Puffer groups conversations by the strongest stable New Relic identifier
present:

- `newrelic:issue:<issue-id>`
- `newrelic:incident:<incident-id>`
- `newrelic:event:<trigger-event>`
- `newrelic:event:<title>`

Messages include title, state, priority, condition, policy, entities, incident
ids, account id, details, runbook URL, chart URL, and the New Relic issue or
incident link when present.
