---
name: gmail-browser
description: Configure Gmail Browser with /connect and use Gmail ConnectorAct actions through the global Puffer browser profile.
allowed-tools:
  - Bash
argument-hint: "[Gmail Browser task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use `/connect gmail-browser <connection>` when the user needs Gmail Browser
setup or auth repair. The flow opens Gmail in the global Puffer browser profile,
lets the user sign in, discovers logged-in Gmail accounts, and saves the
accounts selected for monitoring.

Target: $target

Configuration command:

```text
/connect gmail-browser <connection>
```

Use connector tools for configured Gmail Browser connections instead of opening
a separate browser session. Use `ConnectorActionDraft` for `send_email` so the
human can review the exact outbound email before sending; use `ConnectorAct`
for reads, drafts, and other non-send actions. Gmail Browser uses email-style
action names:

- `list_emails` for recent messages with optional `mailbox`, `category`,
  `label`, `query`, `from`, `subject`, `keywords`, `unread`, and `limit`.
- `list_inbox`, `list_category`, and `search_emails` as read/search variants.
- `mark_read`, `draft_reply`, `draft_forward`, `send_email`, and `delete` for
  message state changes, drafts, sends, and deletion.
- `requestuserbrowseraction` only when the connector needs the user to complete
  sign-in or approval in the global Puffer browser profile.

Do not use `read_messages` for Gmail Browser. That action name belongs to chat
connectors such as Slack and Lark. For Gmail Browser reads, use `list_emails`
or `search_emails`.

Connector action examples:

```json
{"connector_slug":"gmail-browser","connection_slug":"gmail-browser","action":"list_emails","input":{"account":"cs@agentenv.io","limit":20,"unread":true}}
{"connector_slug":"gmail-browser","connection_slug":"gmail-browser","action":"search_emails","input":{"account":"cs@agentenv.io","query":"from:alice newer:7d","limit":20}}
{"connector_slug":"gmail-browser","connection_slug":"gmail-browser","action":"draft_reply","input":{"account":"cs@agentenv.io","thread_id":"thread-a:r123","body":"Thanks, looking now."}}
```

When a connection monitors multiple accounts, include `account` with the Gmail
address to select the mailbox. If omitted, Gmail Browser uses the first
configured account for that connection.
