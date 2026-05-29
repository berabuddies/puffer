---
name: lark
description: Configure LarkApp or LarkLogin with /connect, then resolve Lark chats/users and search/read Lark messages through the internal CLI.
allowed-tools:
  - Bash
argument-hint: "[Lark task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use `/connect lark-app <connection>` or `/connect lark-login <connection>`
when the user needs Lark connector setup, account setup, or auth repair. That
flow uses AskUserQuestion for method choices and secrets.

Use Bash to run the Lark internal CLI only after auth exists, when the user
needs Lark id lookup or message search/read workflows. Run Lark lookup
commands as `lark ...` inside Bash.

Target: $target

Connection/account selection:

Lark has two first-party connector templates:

- `lark-app`: a custom app connection using app_id/app_secret and tenant access
  tokens. Use it for bot-style sends through OpenAPI.
- `lark-login`: a user access token connection. Use it for user-visible
  searches, user sends, and local account operations.

The default connection slug is `lark-app` for `configure-app`, and `lark-login`
for login/lookup commands. Use `--brand lark` for international Lark and
`--brand feishu` for Feishu. When the user has multiple tenants or accounts,
use a distinct kebab-case connection slug and pass it to every command with
`--connection` or `--account`.

Lark workflow subscriptions and agent proxy mode are not implemented yet for
these first-party templates. Do not create Lark workflows that rely on inbound
Lark events until a typed Lark subscribe runtime exists.

```bash
lark --connection work-login search-chats "deploys"
lark --connection work-login search-users --query "Tony" --has-chatted
```

When `/connect` uses the environment import method, it reads `LARK_APP_ID`
plus `LARK_APP_SECRET` for `lark-app`, or `LARK_USER_ACCESS_TOKEN` for
`lark-login`. It honors `LARK_BRAND` unless an explicit brand is supplied.

After `/connect` auth completes, the auth tool automatically registers the
connection with either `connector_slug="lark-app"` or
`connector_slug="lark-login"`. Use the same connection slug in `ConnectorAct`.

Lookup workflow:

When the user names a Lark group, chat, or person, resolve the stable Lark id
before acting. Do not send to a display name directly.

```bash
lark --connection work-login search-chats "deploys"
lark --connection work-login search-users --query "Tony" --has-chatted
```

Use returned `chat_id` values such as `oc_...` as group targets. Use returned
`open_id` values such as `ou_...` as user targets. If you already have a raw id,
you can pass it directly to `ConnectorAct`.

Message read and search workflow:

Use `read-messages` when you already have a `chat_id`; pass `--thread-id`
instead when you need replies from a specific Lark thread. Use
`search-messages` when the user asks to find text across Lark. Use
`mget-messages` when search returns ids and you need full message details.

```bash
lark --connection work-login read-messages --chat-id oc_xxx --page-size 20
lark --connection work-login read-messages --thread-id omt_xxx --page-size 20
lark --connection work-login search-messages "karen" --chat-ids oc_xxx --page-size 20
lark --connection work-login mget-messages --message-ids om_xxx,om_yyy
```

Connector action workflow:

Use `ConnectorAct` for outbound Lark side effects. For messages, pass `to`
with an `oc_...` chat id or `ou_...` open id, plus `message` or `caption`.
To reply, pass `reply_to` or `reply_to_message_id` with an `om_...` message id.
To send local files/images, include `media`, `file`, `files`, `image`, or `path`.
Captions are sent as a text message before the uploaded media because Lark IM
file/image messages carry media keys rather than captions.

```json
{"connector_slug":"lark-login","connection_slug":"work-login","action":"send_message","input":{"to":"ou_xxx","message":"gm"}}
{"connector_slug":"lark-app","connection_slug":"work-app","action":"send_message","input":{"to":"oc_xxx","message":"done","reply_to":"om_xxx"}}
{"connector_slug":"lark-login","connection_slug":"work-login","action":"send_message","input":{"to":"oc_xxx","media":{"path":"/tmp/report.pdf","caption":"report"}}}
```

Use Lark reaction actions after reading/searching messages for the exact
`message_id`. Adding a reaction takes an `emoji_type`; common ASCII aliases
like `thumbsup` are normalized. Removing a reaction requires Lark's
`reaction_id`.

```json
{"connector_slug":"lark-login","connection_slug":"work-login","action":"react","input":{"message_id":"om_xxx","emoji_type":"THUMBSUP"}}
{"connector_slug":"lark-login","connection_slug":"work-login","action":"remove_reaction","input":{"message_id":"om_xxx","reaction_id":"reaction_xxx"}}
```
