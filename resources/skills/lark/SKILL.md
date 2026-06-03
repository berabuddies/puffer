---
name: lark
description: Authenticate Lark with /connect (delegated to the official lark-cli), then send/react/reply through the connector and consume incoming Lark messages as a stream. Two connectors mirror Telegram — lark-login (your user account; monitor + act as you) and lark-bot (the app bot; auto-reply). Use lark-cli directly from Bash for chat/user lookup and any API not exposed as a connector action.
allowed-tools:
  - Bash
argument-hint: "[Lark task]"
arguments: target
user-invocable: false
disable-model-invocation: false
---

The Lark connectors are backed by the official
[larksuite/cli](https://github.com/larksuite/cli) (`lark-cli`). Puffer shells out
to it for every operation and never stores Lark tokens — `lark-cli` keeps its own
OAuth credentials in the OS keychain.

Target: $target

## Two connectors (mirror Telegram login/bot)

| Connector | lark-cli identity | Streams | Auto-reply | Use |
|---|---|---|---|---|
| **lark-login** | `--as user` (you) | yes | no | watch your messages → tasks; send/act as you |
| **lark-bot** | `--as bot` (app bot) | yes | yes | auto-responding chatbot |

Both share the same actions; they differ in identity and whether they auto-reply.
Default connection slugs: `lark-user` and `lark-bot`.

## Authentication

Run `/connect lark-login <connection>` or `/connect lark-bot <connection>`. The
flow verifies `lark-cli auth status` and records the connection; it never prompts
for tokens, and offers to install `lark-cli` (`npx @larksuite/cli@latest install`)
if missing. Log in once in a terminal if needed:

```bash
lark-cli auth login      # interactive OAuth, picks scopes
lark-cli auth status     # confirm a logged-in account
```

`LARK_CLI_BIN` overrides the binary; `LARK_CLI_AS` overrides the event identity.

## Sending / reacting / replying (connector actions)

Use `ConnectorAct` (or a workflow) on the connection. Actions: `send_message`,
`react` / `send_reaction`, `remove_reaction`. Identity defaults to the
connector's (`user` for lark-login, `bot` for lark-bot); override per call with
`as: "bot"|"user"`.

- `send_message`: recipient `chat_id` (`oc_...`→`--chat-id`) or
  `open_id`/`user_id`/`user` (→`--user-id`); a generic `to`/`target` is inferred
  by `oc_` prefix. Body `text` (aliases `message`/`caption`). Media via
  `image`/`file`/`audio` (file key, URL, or cwd-relative path). Reply via
  `reply_to` (a `om_...` id, or `{message_id}`) → `lark-cli im +messages-reply`
  (`reply_in_thread: true` for a thread reply).
- `react` / `send_reaction`: `{message_id, emoji_type}` (EmojiType e.g. `SMILE`,
  `THUMBSUP`, `DONE`).
- `remove_reaction`: `{message_id, reaction_id}`.

## Incoming messages (stream)

Both connectors stream newly received messages as workflow/monitor events; each
payload carries `text`, `message_id`, `chat_id`, `sender_open_id`, `create_time`,
`message_type`. Produced by `lark-cli event consume im.message.receive_v1 --as
bot` — `im.message.receive_v1` is an app-level stream and `event consume` is
bot-only, so both connectors (including lark-login) consume it as the bot; it
covers messages in chats where the app is present. Lark's websocket only delivers
new messages (no resumable cursor), so it starts from "now". Use `/monitor
<connection>` to turn messages into tasks.

## Auto-reply (lark-bot only)

`lark-bot` (`can_proxy_agent = true`) replies automatically. With a running
Puffer runtime (TUI or `puffer daemon`) and the stream active, message the bot
`/connect <agent-slug>` once to bind the chat to a persona; later messages are
answered by that agent (it IS the agent and replies directly), sent back to the
chat as the bot. `lark-login` never auto-replies — it only monitors / acts on
demand.

## Lookups and other APIs

For chat/user discovery or anything not exposed as a connector action, call
`lark-cli` directly from Bash:

```bash
lark-cli im +chat-list --types group        # list chats
lark-cli im +chat-search --query "team"      # resolve a chat id by name
lark-cli api POST /open-apis/im/v1/messages --params '{"receive_id_type":"chat_id"}' \
  --data '{"receive_id":"oc_xxx","msg_type":"text","content":"{\"text\":\"hi\"}"}'
```

Never print Lark secrets or access tokens in output, logs, or event payloads.
