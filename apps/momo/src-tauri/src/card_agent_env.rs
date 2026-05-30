//! Installs the agent-side assets the `momo-card` reveal flow needs:
//!   1. A project ACL rule (`100 allow bash argv momo-card`) so the bin runs
//!      through the puffer permission gate without escalation.
//!   2. A user-level `pay-with-card` skill that tells the agent to call the
//!      bin as a SINGLE command and that it only ever receives last4 + expiry.
//!
//! Both are idempotent: the ACL line is added only if absent; the SKILL.md is
//! overwritten with the canonical text.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

const ACL_LINE: &str = "100 allow bash argv momo-card";

const SKILL_NAME: &str = "pay-with-card";

const SKILL_MD: &str = r#"---
name: pay-with-card
description: Reveal the user's saved U-card for a payment the current task requires. Use ONLY when a step genuinely needs the user's card (a checkout, a deposit/guarantee, a paid booking that asks for a card). Returns just the last 4 digits and expiry — the full card number and CVV are handled securely by the app and are never shown to you.
metadata:
  author: momo
  version: "1.0.0"
  clawdbot:
    emoji: "💳"
---

# Pay with Card

When the task you are doing needs the user's payment card, run this **single**
command (no pipes, no `&&`, no redirection):

```bash
momo-card reveal
```

It prints a JSON summary and nothing else, e.g.:

```json
{"last4": "1234", "expiry": "04/2028"}
```

## What this does (and does not) give you

- You receive **only** `last4` + `expiry`. This is by design and is enough to
  confirm to the user which card was used ("paid with the card ending 1234").
- You will **never** receive the full card number or CVV. The `momo-card` bin
  reveals them inside its own process, uses them, and discards them. Do not ask
  the user for the full number or CVV, and do not try to reconstruct them.
- If the command fails with "sign in to momo first", tell the user to sign in.

## When to use

- A checkout / payment form the task must complete.
- A booking or reservation that requires a card to hold or guarantee it.
- A deposit/top-up that needs a card.

Do **not** run it speculatively — only when a concrete payment step requires it.
"#;

/// Append `100 allow bash argv momo-card` to `<project_cwd>/.puffer/permissions.acl`
/// if an identical line isn't already present. Creates the dir/file as needed.
pub fn ensure_acl_grant(project_cwd: &Path) -> Result<()> {
    let dir = project_cwd.join(".puffer");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("permissions.acl");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == ACL_LINE) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // Ensure we start on a fresh line if the file didn't end with one.
    if !existing.is_empty() && !existing.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    f.write_all(ACL_LINE.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Write the canonical SKILL.md to `<user_puffer>/resources/skills/pay-with-card/SKILL.md`.
pub fn install_skill(user_puffer: &Path) -> Result<()> {
    let dir = user_puffer
        .join("resources")
        .join("skills")
        .join(SKILL_NAME);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, SKILL_MD).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acl_grant_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        ensure_acl_grant(cwd).unwrap();
        ensure_acl_grant(cwd).unwrap();
        let body = std::fs::read_to_string(cwd.join(".puffer").join("permissions.acl")).unwrap();
        assert_eq!(body.matches(ACL_LINE).count(), 1);
    }

    #[test]
    fn acl_grant_preserves_existing_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let dir = cwd.join(".puffer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("permissions.acl"), "200 allow read dir foo").unwrap();
        ensure_acl_grant(cwd).unwrap();
        let body = std::fs::read_to_string(dir.join("permissions.acl")).unwrap();
        assert!(body.contains("200 allow read dir foo"));
        assert!(body.contains(ACL_LINE));
    }

    #[test]
    fn install_skill_writes_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        install_skill(tmp.path()).unwrap();
        let path = tmp
            .path()
            .join("resources/skills/pay-with-card/SKILL.md");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("name: pay-with-card"));
        assert!(body.contains("momo-card reveal"));
    }
}
