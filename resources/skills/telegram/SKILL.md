---
name: telegram
description: Log in the Telegram personal-account connector, resolve Telegram user/group ids, and search Telegram messages through the internal CLI.
allowed-tools:
  - Bash
argument-hint: "[Telegram login task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use Bash to run the Telegram internal CLI when the user needs Telegram
personal-account workflows or asks to log in to Telegram. Telegram is not
a model tool and must not be requested as a provider tool call. Run Telegram
commands as `telegram ...` inside Bash.

Target: $target

Connection/account selection:

The default Telegram personal account connection is `telegram-user`. When the
user has multiple local Telegram accounts, use a distinct kebab-case connection
slug per account and pass it to every Telegram CLI command with `--connection`
or `--account`. Each slug maps to a separate supervised subscriber, topic, and
session file. `--account-index` is only the local import-time picker for
Telegram Desktop/native storage slots; it is not the stable account identity.

```bash
telegram --connection tg-main import-desktop --account-index 0
telegram --account tg-alt import-desktop --account-index 1
telegram --account tg-alt search-peers "C & Jason" --kind group
```

After a login or import completes, the internal tool registers that
connection automatically with `connector_slug="telegram-login"`. Use the
same connection slug in `WorkflowCreate` and `ConnectorAct`.

Peer lookup workflow:

When the user names a Telegram user, group, or channel and the next action
needs a send target or workflow filter, resolve the stable numeric id first.
Do not send to an ambiguous title directly.

```bash
telegram search-peers "C & Jason" --kind group
```

Use `telegram list-peers --kind group --limit 50` to browse visible groups,
or omit `--kind` to include users, groups, and channels. Results are JSON.
Use `payload.peers[].id` as the send target or workflow chat id; it is a
string on purpose so large Telegram ids stay exact.

Message search workflow:

When the user asks to find text in Telegram, resolve the chat first and then
search messages by peer id. Prefer numeric peer ids over titles.

```bash
telegram search-peers "TonyKe" --kind user
telegram search-messages "karen" --peer 123456789 --context 2 --limit 10 --succint
```

Use `--succint` for normal agent work. It returns plain text context lines
with relative offsets, for example `+2 Sender: message`, instead of JSON.
Messages with downloadable media include a local file path; captions follow
the path on the same line. Text-only media such as polls are rendered as
text with answer indexes, for example
`poll: Ship it? [open, 3 voters] | 0: Yes / 1: No`. Replies are shown as a
prefix on the message line, for example `[reply to #42: previous text]`.
Without `--succint`, results are JSON; inspect `payload.results[].context`,
where the message with `is_match: true` is the search hit and neighboring
items are surrounding context. JSON `media` is a string with the same
contract: local file path for downloaded media, or text for text-only media.
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
indexes from `--succint` output or `option_hex` from JSON output; exact answer
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

QR login workflow:

If the user has any logged-in Telegram app, prefer QR login before asking for
a phone code:

```bash
telegram login-qr
```

Show the returned `tg://login?token=...` URL to the user. They should open it
from a logged-in Telegram app and approve the login. Then run:

```bash
telegram login-qr-wait
```

If `login-qr-wait` returns a refreshed QR URL instead of `complete`, show the
new URL and run `telegram login-qr-wait` again after approval.

Desktop import workflow:

If the user has Telegram Desktop with a `tdata` directory, or native macOS
Telegram.app local storage on this machine, importing that local session can
avoid QR/phone login:

```bash
telegram import-desktop
```

Use `--path /path/to/tdata` when Telegram Desktop uses a non-default data
directory. On macOS, omitting `--path` can import native Telegram.app storage
if Telegram Desktop `tdata` is absent. Use `--account-index N` for a
secondary account. When importing multiple local accounts, also pass a unique
connection slug:

```bash
telegram --connection tg-main import-desktop --account-index 0
telegram --connection tg-alt import-desktop --account-index 1
```

If Telegram Desktop has a local passcode, ask for it and prefer:

```bash
telegram import-desktop --passcode-stdin
```

The passcode is the Telegram Desktop local app passcode, not the Telegram
cloud 2FA password.

Login workflow:

If QR login and Telegram Desktop import are unavailable or fail, fall back to
the interactive login:

1. Ask the user for their phone number in E.164 format, including the leading
   `+`, then run:

```bash
telegram login-start +15551234567
```

2. Telegram sends a numeric code to the user's Telegram apps. Ask the user for
   that code, then run:

```bash
telegram login-submit-code 12345
```

3. If the command reports that Telegram requires a 2FA cloud password, ask the
   user for it and run:

```bash
telegram login-submit-password --password '<2FA password>'
```

Only pass `--api-id` and `--api-hash` to `telegram login-qr` or
`telegram login-start` if the user explicitly provides their own Telegram
application credentials. Puffer uses a built-in public Telegram Desktop
credential pair otherwise.

Treat login codes, local passcodes, and 2FA passwords as secrets. Prefer
stdin-based flags when a secret source can be piped into the command;
otherwise use the direct flag for a single non-interactive Bash call. Do not
echo secrets in the final answer, and do not write them to project files.
After login or import completes, use the returned `connection_slug` in
workflows and connector actions when the user wants ongoing monitoring or
outbound side effects.
