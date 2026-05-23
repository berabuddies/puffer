---
name: email
description: Configure the Email connector through the internal CLI so workflows can receive email events.
allowed-tools:
  - Bash
argument-hint: "[email configuration task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use Bash to run the Email internal CLI when the user needs email workflows
or asks to configure email. Email is not a model tool and must not be requested
as a provider tool call. Run Email commands as `email ...` inside Bash.

Target: $target

Collect these values from the user before configuring:

1. IMAP host, for example `imap.gmail.com`, and optional IMAP port.
2. SMTP host, for example `smtp.gmail.com`, and optional SMTP port.
3. Username, usually the full email address.
4. Password or app-specific password.
5. From address for outbound email.
6. Optional allowed sender addresses or domain suffixes such as `@example.com`.

Configuration command:

```bash
email configure \
  --imap-host imap.gmail.com \
  --smtp-host smtp.gmail.com \
  --username alice@example.com \
  --password '<app password>' \
  --from-address alice@example.com
```

Optional flags:

- `--imap-port 993`
- `--smtp-port 587`
- `--password-stdin` when the password is available on stdin
- `--allowed-sender person@example.com`
- `--allowed-sender @example.com`

Treat passwords as secrets. Prefer `--password-stdin` when a secret source can
be piped into the command; otherwise use `--password` for a single
non-interactive Bash call. Do not echo the password in the final answer, and do
not write it to project files. After configuration succeeds, create a
connection with `connector_slug="email"` and then create a workflow with
`connection_slug` set to that connection when the user wants ongoing monitoring.
