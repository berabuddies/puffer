//! 1Password resolve-on-demand support.
//!
//! Rather than copying 1Password secrets into Puffer's vault (which would defeat
//! 1Password's central rotation/revocation/audit), Puffer stores only a
//! `op://vault/item/field` *reference* as a secret value. At the moment an agent
//! requests the secret, the reference is resolved to the live value via the
//! 1Password CLI (`op read`) and handed to the masking layer — so the plaintext
//! never persists in Puffer and never reaches the model directly.
//!
//! `op` reads its credentials from the environment (`OP_SERVICE_ACCOUNT_TOKEN`
//! for headless/daemon use), so no token handling lives here.

use anyhow::{bail, Context, Result};
use std::process::Command;

const OP_REFERENCE_PREFIX: &str = "op://";

/// Returns whether a stored secret value is a 1Password secret reference.
pub fn is_op_reference(value: &str) -> bool {
    value.trim_start().starts_with(OP_REFERENCE_PREFIX)
}

/// Reports whether the 1Password CLI (`op`) is available on this machine.
pub fn op_cli_available() -> bool {
    Command::new("op")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Resolves a `op://vault/item/field` reference to its live value via `op read`.
pub fn resolve_op_reference(reference: &str) -> Result<String> {
    resolve_with("op", reference)
}

/// Resolution core parameterized by the CLI binary, for testing with a fake `op`.
fn resolve_with(op_bin: &str, reference: &str) -> Result<String> {
    let reference = reference.trim();
    if !is_op_reference(reference) {
        bail!("`{reference}` is not a 1Password secret reference (op://...)");
    }
    let output = Command::new(op_bin)
        .args(["read", "--no-newline", reference])
        .output()
        .with_context(|| {
            "run 1Password CLI `op read` (is `op` installed and OP_SERVICE_ACCOUNT_TOKEN set?)"
                .to_string()
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`op read {reference}` failed: {}", stderr.trim());
    }
    let value = String::from_utf8(output.stdout).context("1Password value is not UTF-8")?;
    // `--no-newline` should prevent a trailing newline, but strip defensively.
    let value = value
        .strip_suffix('\n')
        .map(str::to_string)
        .unwrap_or(value);
    if value.is_empty() {
        bail!("1Password reference `{reference}` resolved to an empty value");
    }
    Ok(value)
}

/// One 1Password login surfaced as an importable reference (no plaintext).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpLogin {
    /// `op://vault/item/password` reference resolved on demand at request time.
    pub reference: String,
    /// Display label (the item title).
    pub label: String,
    /// Optional non-secret origin URL.
    pub origin: Option<String>,
}

/// Lists 1Password Login items as `op://` references via the CLI.
pub fn import_login_references() -> Result<Vec<OpLogin>> {
    import_login_with("op")
}

/// Import core parameterized by the CLI binary, for testing with a fake `op`.
fn import_login_with(op_bin: &str) -> Result<Vec<OpLogin>> {
    let output = Command::new(op_bin)
        .args(["item", "list", "--categories", "Login", "--format", "json"])
        .output()
        .with_context(|| {
            "run 1Password CLI `op item list` (is `op` installed and OP_SERVICE_ACCOUNT_TOKEN set?)"
                .to_string()
        })?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`op item list` failed: {}", stderr.trim());
    }
    parse_login_list(&output.stdout)
}

/// Parses `op item list --format json` output into importable login references.
fn parse_login_list(stdout: &[u8]) -> Result<Vec<OpLogin>> {
    let items: serde_json::Value =
        serde_json::from_slice(stdout).context("parse `op item list` JSON")?;
    let array = items
        .as_array()
        .context("`op item list` did not return a JSON array")?;
    let mut out = Vec::new();
    for item in array {
        let (Some(id), Some(vault_id)) = (
            item.get("id").and_then(|value| value.as_str()),
            item.get("vault")
                .and_then(|vault| vault.get("id"))
                .and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        let label = item
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("1Password item")
            .to_string();
        // Reference by stable ids; resolves to the item's password field.
        let reference = format!("op://{vault_id}/{id}/password");
        let origin = item
            .get("urls")
            .and_then(|urls| urls.as_array())
            .and_then(|urls| {
                urls.iter()
                    .find(|url| {
                        url.get("primary")
                            .and_then(|primary| primary.as_bool())
                            .unwrap_or(false)
                    })
                    .or_else(|| urls.first())
            })
            .and_then(|url| url.get("href"))
            .and_then(|href| href.as_str())
            .map(str::to_string);
        out.push(OpLogin {
            reference,
            label,
            origin,
        });
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn write_fake_op(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
        let path = dir.join("op");
        fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    #[test]
    fn recognizes_op_references() {
        assert!(is_op_reference("op://Private/GitHub/credential"));
        assert!(is_op_reference("  op://vault/item/field"));
        assert!(!is_op_reference("ghp_realtoken"));
        assert!(!is_op_reference("https://example.com"));
    }

    #[test]
    fn resolves_reference_via_cli() {
        let dir = tempfile::tempdir().unwrap();
        // Fake `op` echoes a fixed value (no trailing newline, like --no-newline).
        let op = write_fake_op(dir.path(), "printf '%s' 's3cr3t-from-op'");
        let value = resolve_with(op.to_str().unwrap(), "op://Private/GitHub/credential").unwrap();
        assert_eq!(value, "s3cr3t-from-op");
    }

    #[test]
    fn surfaces_cli_failure() {
        let dir = tempfile::tempdir().unwrap();
        let op = write_fake_op(dir.path(), "echo 'no such item' 1>&2; exit 1");
        let err = resolve_with(op.to_str().unwrap(), "op://x/y/z").unwrap_err();
        assert!(err.to_string().contains("failed"));
    }

    #[test]
    fn rejects_non_reference() {
        assert!(resolve_with("op", "not-a-ref").is_err());
    }

    #[test]
    fn parses_login_list_into_references() {
        let json = br#"[
            {"id":"abc123","title":"GitHub","vault":{"id":"vlt1","name":"Private"},
             "category":"LOGIN","urls":[{"primary":true,"href":"https://github.com"}]},
            {"id":"def456","title":"No URL","vault":{"id":"vlt1","name":"Private"},"category":"LOGIN"}
        ]"#;
        let logins = parse_login_list(json).unwrap();
        assert_eq!(logins.len(), 2);
        assert_eq!(logins[0].reference, "op://vlt1/abc123/password");
        assert_eq!(logins[0].label, "GitHub");
        assert_eq!(logins[0].origin.as_deref(), Some("https://github.com"));
        assert_eq!(logins[1].reference, "op://vlt1/def456/password");
        assert_eq!(logins[1].origin, None);
    }

    #[test]
    fn imports_login_list_via_cli() {
        let dir = tempfile::tempdir().unwrap();
        let op = write_fake_op(
            dir.path(),
            r#"printf '%s' '[{"id":"i1","title":"Acme","vault":{"id":"v1"},"urls":[{"primary":true,"href":"https://acme.test"}]}]'"#,
        );
        let logins = import_login_with(op.to_str().unwrap()).unwrap();
        assert_eq!(logins.len(), 1);
        assert_eq!(logins[0].reference, "op://v1/i1/password");
        assert_eq!(logins[0].origin.as_deref(), Some("https://acme.test"));
    }
}
