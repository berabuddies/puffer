---
name: telegram
description: Log in the Telegram personal-account subscriber through the internal CLI so subscriptions can receive Telegram events.
allowed-tools:
  - Bash
argument-hint: "[Telegram login task]"
arguments: target
user-invocable: true
disable-model-invocation: false
---

Use Bash to run the Telegram internal CLI when the user needs Telegram
personal-account subscriptions or asks to log in to Telegram. Telegram is not
a model tool and must not be requested as a provider tool call. Run Telegram
commands as `telegram ...` inside Bash.

Target: $target

Login workflow:

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

Only pass `--api-id` and `--api-hash` to `telegram login-start` if the user
explicitly provides their own Telegram application credentials. Puffer uses a
built-in public Telegram Desktop credential pair otherwise.

Treat login codes and 2FA passwords as secrets. Prefer `--password-stdin` for
the 2FA password when a secret source can be piped into the command; otherwise
use `--password` for a single non-interactive Bash call. Do not echo secrets in
the final answer, and do not write them to project files. After login
completes, install a subscription with `source_topic="telegram-user"` when the
user wants ongoing monitoring.
