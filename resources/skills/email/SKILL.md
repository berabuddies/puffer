---
name: email
description: Configure the Email connector with /connect so workflows can receive email events.
allowed-tools:
  - Bash
argument-hint: "[email configuration task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use `/connect email <connection>` when the user needs email workflow setup or
auth repair. That flow uses AskUserQuestion for hosts, username, app password,
from address, and allowed sender choices.

Target: $target

The `/connect` flow collects these values before configuring:

1. IMAP host, for example `imap.gmail.com`, and optional IMAP port.
2. SMTP host, for example `smtp.gmail.com`, and optional SMTP port.
3. Username, usually the full email address.
4. Password or app-specific password.
5. From address for outbound email.
6. Optional allowed sender addresses or domain suffixes such as `@example.com`.

Configuration command:

```text
/connect email <connection>
```

Treat passwords as secrets. Do not echo the password in the final answer, and
do not write it to project files. After configuration succeeds, create a
workflow with `connection_slug` set to that connection when the user wants
ongoing monitoring.
