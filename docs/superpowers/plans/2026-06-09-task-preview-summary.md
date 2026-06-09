# Task Preview Summary Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make monitor task history previews content-first while moving source details into metadata.

**Architecture:** The daemon keeps the stable `summary` response field, but `summary` becomes a cleaned content preview. Desktop renders the preview in the existing row body and moves connector, scope, sender, delivery, lag, and index fields into the metadata line.

**Tech Stack:** Rust daemon code in `puffer-cli`, Svelte desktop UI, Playwright desktop UI tests, Cargo tests.

---

## File Structure

- Modify `crates/puffer-cli/src/daemon_workflows/monitor_history.rs`: add preview classification helpers and unit tests.
- Modify `apps/puffer-desktop/src/lib/screens/Tasks.svelte`: render source details in history metadata instead of relying on source-prefixed summaries.
- Modify `apps/puffer-desktop/tests/support/fakeDaemon.ts`: update history fixtures to use content-only summaries.
- Modify `apps/puffer-desktop/tests/tasks-ui.spec.ts`: assert content preview and source metadata separately.
- Create `specs/puffer-cli/174.md`: daemon summary contract update spec.
- Create `specs/puffer-desktop/695.md`: desktop history metadata update spec. `694.md` exists as an uncommitted file in the main workspace, so use `695.md` in this branch.

---

### Task 1: Backend Summary Tests

**Files:**
- Modify: `crates/puffer-cli/src/daemon_workflows/monitor_history.rs`

- [ ] **Step 1: Add failing tests for content-only monitor previews**

Append this module to `monitor_history.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::message_summary;
    use serde_json::json;

    #[test]
    fn message_summary_prefers_body_over_source_fields() {
        let payload = json!({
            "chat_title": "Support",
            "sender_username": "alice",
            "message": "deployment status?"
        });

        assert_eq!(message_summary(&payload, ""), "deployment status?");
    }

    #[test]
    fn message_summary_strips_telegram_source_prefixes() {
        let payload = json!({
            "summary": "Telegram message from Alice: deployment status?"
        });

        assert_eq!(message_summary(&payload, ""), "deployment status?");
    }

    #[test]
    fn message_summary_extracts_core_error_text() {
        let payload = json!({
            "message": "Incoming Telegram message from Alice: Error: deployment failed in production"
        });

        assert_eq!(
            message_summary(&payload, ""),
            "deployment failed in production"
        );
    }

    #[test]
    fn message_summary_keeps_question_intent() {
        let payload = json!({
            "channel_name": "#ops",
            "author_handle": "sam",
            "text": "Can you check the failing deploy?"
        });

        assert_eq!(message_summary(&payload, ""), "Can you check the failing deploy?");
    }

    #[test]
    fn message_summary_keeps_request_action() {
        let payload = json!({
            "from_email": "ops@example.com",
            "body": "please restart the staging worker when the deploy is done"
        });

        assert_eq!(
            message_summary(&payload, ""),
            "restart the staging worker when the deploy is done"
        );
    }

    #[test]
    fn message_summary_falls_back_to_short_clean_content() {
        let payload = json!({
            "body": "This is a casual note with enough words that the preview should stay short and readable for the sidebar row."
        });

        assert_eq!(
            message_summary(&payload, ""),
            "This is a casual note with enough words that the preview should stay short and..."
        );
    }
}
```

- [ ] **Step 2: Run the focused backend test and confirm it fails**

Run:

```bash
cargo test -p puffer-cli message_summary
```

Expected: FAIL. At least `message_summary_prefers_body_over_source_fields` should show the old `Support from alice: deployment status?` source-first output.

---

### Task 2: Backend Summary Implementation

**Files:**
- Modify: `crates/puffer-cli/src/daemon_workflows/monitor_history.rs`

- [ ] **Step 1: Replace source-first summary helpers with content-first helpers**

Replace `SUMMARY_LIMIT`, `message_summary`, and `truncate_summary` with this block, keeping the existing `string_field` and `first_payload_string` helper names:

```rust
const SUMMARY_LIMIT: usize = 80;

fn message_summary(payload: &Value, text: &str) -> String {
    let candidate = first_payload_string(
        payload,
        &[
            "message",
            "snippet",
            "text",
            "body",
            "subject",
            "title",
            "event_title",
            "summary",
        ],
    )
    .unwrap_or_else(|| text.trim().to_string());
    let cleaned = strip_source_prefixes(&normalize_inline_text(&candidate));
    let preview = error_preview(&cleaned)
        .or_else(|| question_preview(&cleaned))
        .or_else(|| request_preview(&cleaned))
        .unwrap_or(cleaned);
    truncate_summary(&preview)
}

fn strip_source_prefixes(value: &str) -> String {
    let mut current = value.trim().to_string();
    for _ in 0..4 {
        let next = strip_one_source_prefix(&current);
        if next == current {
            break;
        }
        current = next;
    }
    current
}

fn strip_one_source_prefix(value: &str) -> String {
    let text = value.trim();
    let lower = text.to_ascii_lowercase();
    for prefix in [
        "telegram message from ",
        "incoming telegram message from ",
        "incoming telegram message",
        "telegram user sent",
        "in telegram group ",
        "message from chat_id ",
    ] {
        if lower.starts_with(prefix) {
            return strip_prefix_subject(&text[prefix.len()..]);
        }
    }
    strip_generic_colon_source(text).unwrap_or_else(|| text.to_string())
}

fn strip_prefix_subject(value: &str) -> String {
    let rest = value
        .trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, ':' | '-' | ','));
    if let Some((_, tail)) = rest.split_once(':') {
        return tail.trim().to_string();
    }
    rest.trim().to_string()
}

fn strip_generic_colon_source(value: &str) -> Option<String> {
    let (head, tail) = value.split_once(':')?;
    if head.chars().count() > 80 {
        return None;
    }
    let lower = head.to_ascii_lowercase();
    let looks_like_source = [
        "telegram", "slack", "discord", "lark", "email", "mail", "chat_id", "channel",
        "room", "group", " from ",
    ]
    .iter()
    .any(|marker| lower.contains(marker));
    looks_like_source.then(|| tail.trim().to_string())
}

fn error_preview(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let mut earliest = None;
    for marker in [
        "error:",
        "failed:",
        "failure:",
        "fatal:",
        "panic:",
        "exception:",
        "traceback",
        "could not ",
        "cannot ",
    ] {
        if let Some(index) = lower.find(marker) {
            earliest = Some(earliest.map_or(index, |current: usize| current.min(index)));
        }
    }
    let index = earliest?;
    let excerpt = first_sentence(&value[index..]);
    let core = strip_error_label(&excerpt);
    (!core.trim().is_empty()).then_some(core)
}

fn strip_error_label(value: &str) -> String {
    let trimmed = value.trim_start_matches(|ch: char| ch.is_whitespace() || matches!(ch, '-' | ':'));
    if let Some((label, tail)) = trimmed.split_once(':') {
        let lower = label.to_ascii_lowercase();
        if label.chars().count() <= 48
            && ["error", "failed", "failure", "fatal", "panic", "exception", "traceback"]
                .iter()
                .any(|marker| lower.contains(marker))
        {
            return tail.trim().to_string();
        }
    }
    trimmed.to_string()
}

fn question_preview(value: &str) -> Option<String> {
    if let Some(index) = value.find('?') {
        return Some(first_sentence(&value[..=index]));
    }
    let lower = value.to_ascii_lowercase();
    [
        "can you ",
        "could you ",
        "would you ",
        "do you ",
        "is there ",
        "are there ",
        "what ",
        "why ",
        "how ",
        "when ",
        "where ",
        "should ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
    .then(|| first_sentence(value))
}

fn request_preview(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    for prefix in ["please ", "pls "] {
        if lower.starts_with(prefix) {
            return Some(first_sentence(value[prefix.len()..].trim()));
        }
    }
    [
        "need ",
        "need you to ",
        "restart ",
        "fix ",
        "check ",
        "review ",
        "send ",
        "update ",
        "create ",
        "delete ",
        "deploy ",
    ]
    .iter()
    .any(|marker| lower.starts_with(marker))
    .then(|| first_sentence(value))
}

fn first_sentence(value: &str) -> String {
    for separator in [". ", "; ", " | "] {
        if let Some((head, _)) = value.split_once(separator) {
            return head.trim().to_string();
        }
    }
    value.trim().to_string()
}

fn normalize_inline_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn truncate_summary(value: &str) -> String {
    let summary = normalize_inline_text(value);
    if summary.chars().count() <= SUMMARY_LIMIT {
        return summary;
    }
    let truncated = summary.chars().take(SUMMARY_LIMIT).collect::<String>();
    format!("{}...", truncated.trim_end())
}
```

- [ ] **Step 2: Run the backend tests and confirm they pass**

Run:

```bash
cargo test -p puffer-cli message_summary
```

Expected: PASS, with the six `message_summary_*` tests passing.

- [ ] **Step 3: Commit the backend summary change**

Run:

```bash
git add crates/puffer-cli/src/daemon_workflows/monitor_history.rs
git commit -m "fix: make monitor history summaries content-first"
```

---

### Task 3: Desktop History Metadata

**Files:**
- Modify: `apps/puffer-desktop/src/lib/screens/Tasks.svelte`
- Modify: `apps/puffer-desktop/tests/support/fakeDaemon.ts`
- Modify: `apps/puffer-desktop/tests/tasks-ui.spec.ts`

- [ ] **Step 1: Update fake history summaries to content-only values**

In `apps/puffer-desktop/tests/support/fakeDaemon.ts`, change the default monitor history fixture summary:

```ts
summary: "deployment status?",
```

In `apps/puffer-desktop/tests/tasks-ui.spec.ts`, update the two inline monitor history fixtures:

```ts
summary: "duplicate alert",
```

and:

```ts
summary: "can you check this?",
```

- [ ] **Step 2: Add payload string helpers and source metadata labels**

In `apps/puffer-desktop/src/lib/screens/Tasks.svelte`, add these helpers near `numericPayloadValue`:

```ts
  function stringPayloadValue(payload: Record<string, unknown> | null | undefined, key: string): string | null {
    const value = payload?.[key];
    return typeof value === "string" && value.trim().length > 0 ? value.trim() : null;
  }

  function firstPayloadString(payload: Record<string, unknown> | null | undefined, keys: string[]): string | null {
    for (const key of keys) {
      const value = stringPayloadValue(payload, key);
      if (value) return value;
    }
    return null;
  }

  function historySenderLabel(message: WorkflowMonitorHistoryMessage): string | null {
    return firstPayloadString(message.payload, [
      "sender_username",
      "sender_name",
      "from_email",
      "sender_email",
      "author_handle",
      "from",
      "sender",
      "author"
    ]);
  }

  function historyScopeLabel(message: WorkflowMonitorHistoryMessage): string | null {
    return firstPayloadString(message.payload, [
      "chat_title",
      "chat_name",
      "room_name",
      "channel_name",
      "calendar_id",
      "mailbox"
    ]);
  }
```

- [ ] **Step 3: Move source details into the metadata label**

Replace `historyMetaLabel` in `Tasks.svelte` with:

```ts
  function historyMetaLabel(message: WorkflowMonitorHistoryMessage): string {
    return [
      historySourceLabel(message),
      historyScopeLabel(message),
      historySenderLabel(message),
      message.kind ?? "message",
      historyDeliveryLabel(message),
      historyLagLabel(message),
      `#${message.idx}`
    ].filter(Boolean).join(" · ");
  }
```

- [ ] **Step 4: Update Playwright assertions**

In the first task history test in `apps/puffer-desktop/tests/tasks-ui.spec.ts`, replace the old accessible-name assertion with:

```ts
  const receivedMessages = dialog.getByLabel("Received messages");
  await expect(receivedMessages.getByRole("button", {
    name: /deployment status\?/
  })).toBeVisible();
  await expect(receivedMessages).toContainText("telegram-user");
  await expect(receivedMessages).toContainText("Support");
  await expect(receivedMessages).toContainText("alice");
```

In the processing test, replace:

```ts
  await expect(dialog.getByLabel("Received messages")).toContainText("Telegram from Alice");
```

with:

```ts
  await expect(dialog.getByLabel("Received messages")).toContainText("can you check this?");
  await expect(dialog.getByLabel("Received messages")).toContainText("telegram-user");
  await expect(dialog.getByLabel("Received messages")).toContainText("Support");
  await expect(dialog.getByLabel("Received messages")).toContainText("alice");
```

- [ ] **Step 5: Run the focused desktop checks**

Run:

```bash
npm --prefix apps/puffer-desktop run check
npm --prefix apps/puffer-desktop run test:desktop -- tests/tasks-ui.spec.ts
```

Expected: both commands pass.

- [ ] **Step 6: Commit the desktop metadata change**

Run:

```bash
git add apps/puffer-desktop/src/lib/screens/Tasks.svelte apps/puffer-desktop/tests/support/fakeDaemon.ts apps/puffer-desktop/tests/tasks-ui.spec.ts
git commit -m "fix: show task history source details as metadata"
```

---

### Task 4: Component Specs and Final Verification

**Files:**
- Create: `specs/puffer-cli/174.md`
- Create: `specs/puffer-desktop/695.md`

- [ ] **Step 1: Add the puffer-cli update spec**

Create `specs/puffer-cli/174.md`:

```markdown
# Monitor history content previews

- `task_monitor_history_list` keeps the existing `summary` string field, but
  the field now contains a content-first preview rather than connector or sender
  source text.
- Preview generation prefers message body fields over source fields, strips
  common transport prefixes, extracts clear error cores, preserves question or
  request intent, and falls back to a short cleaned content excerpt.
- The response payload, raw text, action log, connector, connection, envelope,
  and timestamp fields remain unchanged for compatibility.
```

- [ ] **Step 2: Add the puffer-desktop update spec**

Create `specs/puffer-desktop/695.md`:

```markdown
# Task history source metadata

- Task history rows continue to render `message.summary` as the main preview,
  but source fields no longer need to be embedded in that preview text.
- The metadata line now includes connection or connector identity, scope,
  sender, message kind, delivery mode, lag, and history index when those values
  are available.
- Existing task history selection, payload inspection, triage outcome display,
  and ignore-analysis linking are unchanged.
```

- [ ] **Step 3: Run final focused verification**

Run:

```bash
cargo test -p puffer-cli message_summary
npm --prefix apps/puffer-desktop run check
npm --prefix apps/puffer-desktop run test:desktop -- tests/tasks-ui.spec.ts
```

Expected: all commands pass.

- [ ] **Step 4: Commit specs**

Run:

```bash
git add specs/puffer-cli/174.md specs/puffer-desktop/695.md
git commit -m "docs: specify task history preview behavior"
```

- [ ] **Step 5: Report final state**

Report:

```text
Implemented content-first task history previews on codex/task-preview-summary.
Verified:
- cargo test -p puffer-cli message_summary
- npm --prefix apps/puffer-desktop run check
- npm --prefix apps/puffer-desktop run test:desktop -- tests/tasks-ui.spec.ts
```
