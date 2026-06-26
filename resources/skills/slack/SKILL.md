---
name: slack
description: Configure SlackApp or SlackLogin with /connect, then resolve Slack users/channels and search/read Slack messages through the internal CLI.
allowed-tools:
  - Bash
argument-hint: "[Slack task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use `/connect slack-app <connection>` or `/connect slack-login <connection>`
when the user needs Slack connector setup, account setup, or auth repair. That
flow uses AskUserQuestion for method choices and secrets.

Use Bash to run the Slack internal CLI only after auth exists, when the user
needs Slack id lookup or message search/read workflows. Run Slack lookup
commands as `slack ...` inside Bash.

Target: $target

Connection/account selection:

Slack has two first-party connector templates:

- `slack-app`: a Slack app connection with bot token and app-level token. Use
  it for app installs and bot-token channel actions.
- `slack-login`: a Slack workspace account connection from an OAuth token or
  browser session. Use it for local account operations and local app imports.

Slack workflow subscriptions and agent proxy mode are not implemented yet for
these first-party templates. Do not create Slack workflows that rely on inbound
Slack events until a typed Slack subscribe runtime exists.

The default connection slug is `slack-app` for `configure-app`, and
`slack-login` for all login/import/lookup commands. When the user has multiple
workspaces or accounts, use a distinct kebab-case connection slug and pass it
to every Slack CLI command with `--connection` or `--account`.

```bash
slack --connection work-login search-conversations "deploys"
slack --connection work-login search-users "Tony"
```

After `/connect` auth completes, the auth tool automatically registers the
connection with either `connector_slug="slack-app"` or `connector_slug="slack-login"`. Use the same
connection slug in `ConnectorActionDraft` for sends and `ConnectorAct` for
non-send actions.

Lookup workflow:

When the user names a Slack channel, group DM, DM, or person, resolve the
stable Slack id before acting. Do not send to an ambiguous display name,
`#channel` name, or `@handle` directly.

```bash
slack --connection work-login search-conversations "deploys"
slack --connection work-login search-users "Tony"
```

Use returned conversation `id` values such as `C...`, `G...`, or `D...` as
send targets. Use returned user `id` values such as `U...` as send targets
when the task is a DM; Puffer opens the Slack DM before sending.

Message read and search workflow:

Use `read-messages` when you already have a channel id and optionally a thread
timestamp. Use `search-messages` when the user asks to find text across Slack
with Slack search syntax.

```bash
slack --connection work-login read-messages --channel C123 --limit 20
slack --connection work-login read-messages --channel C123 --thread-ts 1700000000.000100
slack --connection work-login search-messages "karen in:deploys" --limit 20
```

Connector action workflow:

Use `ConnectorActionDraft` for outbound Slack messages so the human can review
the exact recipient and content before sending. Pass `to` or `channel`, plus
`message` or `caption`. To reply in a thread, pass `thread_ts` or `reply_to`
with the Slack timestamp. To send files, include `media`, `file`, `files`, or
`path` with local file paths. Captions are sent as Slack upload comments. Use
`ConnectorAct` only for non-send side effects such as reactions.

```json
{"connector_slug":"slack-login","connection_slug":"work-login","action":"send_message","input":{"to":"U123","message":"gm"}}
{"connector_slug":"slack-app","connection_slug":"work-app","action":"send_message","input":{"to":"C123","message":"done","thread_ts":"1700000000.000100"}}
{"connector_slug":"slack-login","connection_slug":"work-login","action":"send_message","input":{"to":"C123","media":{"path":"/tmp/report.pdf","caption":"report"}}}
```

Use Slack reaction actions on connector targets after reading/searching
messages for the exact channel and `ts`.

```json
{"connector_slug":"slack-login","connection_slug":"work-login","action":"react","input":{"channel":"C123","ts":"1700000000.000100","emoji":"white_check_mark"}}
{"connector_slug":"slack-login","connection_slug":"work-login","action":"remove_reaction","input":{"channel":"C123","ts":"1700000000.000100","emoji":":eyes:"}}
```
