---
name: telegram
description: Configure Telegram with /connect, then resolve Telegram user/group ids and search Telegram messages through the internal Telegram CLI.
allowed-tools:
  - Bash
argument-hint: "[Telegram login task]"
arguments: target
user-invocable: false
disable-model-invocation: false
---

Use `/connect telegram-login <connection>` when the user needs to authenticate
or repair a Telegram personal-account connection. That flow uses
AskUserQuestion for method choices and secrets.

Use the internal Telegram CLI when peer lookup, message listing, or message
search is needed. Run it from Bash as `telegram ...`; do not expose a
connector-specific slash command for Telegram. The CLI dispatches to the Telegram
subscriber and does not require a model turn.

Target: $target

Connection/account selection:

The default Telegram personal account connection is `telegram-user`. When the
user has multiple local Telegram accounts, use a distinct kebab-case connection
slug per account and pass it to every Telegram CLI command with `--connection`
or `--account`. Each slug maps to a separate supervised subscriber, topic, and
session file. `--account-index` is only the local import-time picker for
Telegram Desktop/native storage slots; it is not the stable account identity.

```bash
telegram --connection tg-alt search-peers "C & Jason" --kind group
```

After `/connect` login or import completes, the auth tool registers that
connection automatically with `connector_slug="telegram-login"`. Use the same
connection slug in `WorkflowCreate` and `ConnectorAct`.

Peer lookup workflow:

When the user names a Telegram user, group, or channel and the next action
needs a send target or workflow filter, resolve the stable numeric id first.
Do not send to an ambiguous title directly.

```bash
telegram search-peers "C & Jason" --kind group
```

Use `telegram list-peers --kind group --limit 50` to browse visible groups,
or omit `--kind` to include users, groups, and channels. The local command
returns the subscriber result directly. Use `payload.peers[].id` as the send
target or workflow chat id when JSON output is returned; it is a string on
purpose so large Telegram ids stay exact.

Message search workflow:

When the user asks to find text in Telegram, resolve the chat first and then
search messages by peer id. Prefer numeric peer ids over titles.

```bash
telegram search-peers "TonyKe" --kind user
telegram list-messages --peer 123456789 --limit 20
telegram list-messages --peer 123456789 --from "TonyKe" --scan-limit 200 --limit 20
telegram search-messages "karen" --peer 123456789 --context 0 --limit 10
```

Use `telegram list-messages` when the user asks to inspect recent chat history
without a specific search term. Use the returned `--before-id` cursor
to fetch older pages. Add `--from <sender>` when the user asks for messages
from one person in a group; the sender filter matches sender id, username,
`@handle`, or display name. The command scans a bounded recent window while
collecting sender matches; increase `--scan-limit` when a sparse sender needs a
deeper page. Use `--context 0` for fast search; non-zero context returns
previous messages before each hit.

Use `--succinct` with the internal CLI for normal agent work. It returns plain
text context lines with relative offsets, for example `-2 Sender: message`,
instead of JSON. Only the message commands accept `--succinct`:
`list-messages` and `search-messages` support it; the peer commands
`list-peers` and `search-peers` do not (they always return their peer list).
Messages with downloadable media include a local file path; captions follow
the path on the same line. Text-only media such as polls are rendered as
text with answer indexes, for example
`poll: Ship it? [open, 3 voters] | 0: Yes / 1: No`. Replies are shown as a
prefix on the message line, for example `[reply to #42: previous text]`.
Without `--succinct`, results are JSON; inspect `payload.results[].context`,
where the message with `is_match: true` is the search hit and earlier items are
previous-message context. JSON `media` is a string with the same contract: local
file path for downloaded media, or text for text-only media.
JSON `poll` is null or an object with question, open/closed status, total
voters, answer indexes, answer text, chosen/correct flags, voter counts, and
`option_hex` tokens. JSON `reply_to` is null or an object with the replied
message id, peer, thread top id, quote text, quote offsets, reply media kind,
forwarded reply metadata, and a one-level `resolved_message` when Telegram can
fetch it.

Connector action workflow:

Use the `send_message` connector action for outbound Telegram messages. Pass
`reply_to` or `reply_to_message_id` with a Telegram message id to send the
outbound message as a reply. To send files, images, or albums, include `media`
as a path/URL string, an object, or an array. The message text is the caption
for the first attachment unless an attachment object has its own `caption`.

```json
{
  "to": "123456789",
  "message": "done",
  "reply_to": 42,
  "media": [
    {"path": "/tmp/screenshot.png", "kind": "photo", "caption": "result"},
    {"path": "/tmp/report.pdf", "kind": "file"}
  ]
}
```

Use Telegram-specific connector actions for message edits/deletes/forwards,
pinning, reactions, read state, poll votes, chat membership/admin operations,
account profile updates, group metadata updates, avatars, and stories. These
are `ConnectorAct` actions on `telegram-login`; do not invent a separate
Telegram runtime command. Prefer numeric peer ids resolved by `search-peers`.

Use the `vote_poll` connector action to click a poll answer. Prefer answer
indexes from `--succinct` output or `option_hex` from JSON output; exact answer
text also works when unambiguous.

```json
{"to": "123456789", "message_id": 77, "option": 0}
```

Common Telegram action examples:

```json
{"connector_slug": "telegram-login", "connection_slug": "tg-alt", "action": "update_group_title", "input": {"to": "123456789", "title": "New group name"}}
{"connector_slug": "telegram-login", "action": "react", "input": {"to": "123456789", "message_id": 77, "emoji": "<emoji>"}}
{"connector_slug": "telegram-login", "action": "pin_message", "input": {"to": "123456789", "message_id": 77}}
{"connector_slug": "telegram-login", "action": "invite_users", "input": {"to": "123456789", "users": ["987654321"]}}
{"connector_slug": "telegram-login", "action": "update_profile", "input": {"first_name": "Puffer", "about": "Agent account"}}
{"connector_slug": "telegram-login", "action": "update_avatar", "input": {"path": "/tmp/avatar.jpg"}}
{"connector_slug": "telegram-login", "action": "update_group_photo", "input": {"to": "123456789", "path": "/tmp/group.jpg"}}
{"connector_slug": "telegram-login", "action": "send_story", "input": {"media": "/tmp/story.jpg", "caption": "build passed"}}
```

Supported action slugs include `vote_poll`, `edit_message`, `delete_messages`,
`forward_messages`, `pin_message`, `unpin_message`, `unpin_all_messages`,
`react`, `mark_read`, `clear_mentions`, `send_typing`, `join_chat`,
`leave_chat`, `invite_users`, `kick_participant`, `ban_participant`,
`unban_participant`, `update_profile`, `update_username`, `update_avatar`,
`update_group_title`, `update_group_name`, `update_group_username`,
`update_group_photo`, and `send_story`. Telegram may reject an action if the
account lacks the required admin rights or the peer type does not support it.

Authentication workflow:

For Telegram Desktop import, QR login, phone login, login codes, local
passcodes, and 2FA passwords, route the user through:

```text
/connect telegram-login <connection>
```

The `/connect` command asks the user to choose import, QR, or phone login with
AskUserQuestion, gathers any required input there, and calls the typed Telegram
auth helper directly. Do not reproduce the auth flow with Bash or stdin flags
from this skill.
After login or import completes, use the returned `connection_slug` in
workflows and connector actions when the user wants ongoing monitoring or
outbound side effects.
