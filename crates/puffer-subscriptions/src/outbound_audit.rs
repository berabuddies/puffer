use serde::Serialize;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

pub const AUDIT_DECISION_ALLOWED_RULE: &str = "allowed_rule";
pub const AUDIT_DECISION_DRAFT_REQUIRED: &str = "draft_required";
pub const AUDIT_DECISION_APPROVED_SEND: &str = "approved_send";
pub const AUDIT_DECISION_CANCELLED: &str = "cancelled";
pub const AUDIT_DECISION_EXPIRED: &str = "expired";

#[derive(Debug, Clone, Serialize)]
pub struct AuditEntry {
    pub origin: String,
    pub connector: String,
    pub action: String,
    pub decision: String,
    pub action_id: Option<String>,
    pub rule_id: Option<String>,
}

#[derive(Serialize)]
struct AuditLine<'a> {
    at_ms: u64,
    #[serde(flatten)]
    entry: &'a AuditEntry,
}

pub fn append_gate_audit(path: &Path, entry: &AuditEntry) {
    if let Err(error) = append_gate_audit_inner(path, entry) {
        eprintln!("outbound audit write failed: {error}");
    }
}

fn append_gate_audit_inner(path: &Path, entry: &AuditEntry) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let line = serde_json::to_string(&AuditLine {
        at_ms: now_ms(),
        entry,
    })
    .map_err(std::io::Error::other)?;
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")?;
    Ok(())
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_one_json_line_per_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("outbound_audit.ndjson");
        for decision in ["draft_required", "allowed_rule"] {
            append_gate_audit(
                &path,
                &AuditEntry {
                    origin: "llm".into(),
                    connector: "telegram-login".into(),
                    action: "send_message".into(),
                    decision: decision.into(),
                    action_id: None,
                    rule_id: None,
                },
            );
        }
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<_> = text.lines().collect();
        assert_eq!(lines.len(), 2);
        let first: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(first["decision"], "draft_required");
        assert!(first["at_ms"].as_u64().unwrap() > 0);
    }

    #[test]
    fn write_failure_does_not_panic() {
        // Point at an unwritable path (a directory used as a file).
        let dir = tempfile::tempdir().unwrap();
        append_gate_audit(
            dir.path(),
            &AuditEntry {
                origin: "llm".into(),
                connector: "x".into(),
                action: "y".into(),
                decision: "draft_required".into(),
                action_id: None,
                rule_id: None,
            },
        ); // passing = not panicking
    }
}
