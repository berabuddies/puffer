use super::secret_value;
use crate::AppState;
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModalActionInput {
    action: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<Value>,
}

/// Executes a Modal CLI operation backed by verified Lambda Skill contracts.
pub fn execute_modal_action(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: ModalActionInput =
        serde_json::from_value(input).context("invalid ModalAction input")?;
    match parsed.action.as_str() {
        "secretCreate" => secret_create(state, cwd, parsed),
        other => bail!("unsupported ModalAction action `{other}`"),
    }
}

fn secret_create(state: &AppState, cwd: &Path, input: ModalActionInput) -> Result<String> {
    let name = required_string(input.name, "name")?;
    validate_modal_secret_name(&name)?;
    let value = input
        .value
        .as_ref()
        .context("ModalAction secretCreate requires value")?;
    let assignment = secret_value::resolve_secret_handle(state, value)?;
    validate_modal_assignment(&assignment)?;
    let output = Command::new("modal")
        .arg("secret")
        .arg("create")
        .arg(&name)
        .arg(&assignment)
        .current_dir(cwd)
        .output()
        .context("failed to run `modal secret create`")?;
    if !output.status.success() {
        let stdout = redact_secret_text(&String::from_utf8_lossy(&output.stdout), &assignment);
        let stderr = redact_secret_text(&String::from_utf8_lossy(&output.stderr), &assignment);
        bail!(
            "modal secret create failed with status {}: stdout={}, stderr={}",
            output.status,
            stdout.trim(),
            stderr.trim()
        );
    }
    Ok("{}".to_string())
}

fn required_string(value: Option<String>, name: &str) -> Result<String> {
    let Some(value) = value else {
        bail!("ModalAction requires {name}");
    };
    if value.trim().is_empty() {
        bail!("ModalAction {name} must be non-empty");
    }
    Ok(value)
}

fn validate_modal_secret_name(name: &str) -> Result<()> {
    static SECRET_NAME: OnceLock<Regex> = OnceLock::new();
    let regex =
        SECRET_NAME.get_or_init(|| Regex::new(r"^[A-Za-z0-9][A-Za-z0-9_.-]{0,127}$").unwrap());
    if regex.is_match(name) {
        return Ok(());
    }
    bail!("ModalAction secret name must contain only letters, numbers, dot, underscore, or dash")
}

fn validate_modal_assignment(assignment: &str) -> Result<()> {
    let Some((key, value)) = assignment.split_once('=') else {
        bail!("ModalAction secret value must be a KEY=VALUE assignment");
    };
    static KEY: OnceLock<Regex> = OnceLock::new();
    let regex = KEY.get_or_init(|| Regex::new(r"^[A-Za-z_][A-Za-z0-9_]*$").unwrap());
    if !regex.is_match(key) {
        bail!("ModalAction secret assignment key is invalid");
    }
    if value.is_empty() {
        bail!("ModalAction secret assignment value must be non-empty");
    }
    Ok(())
}

fn redact_secret_text(text: &str, assignment: &str) -> String {
    let mut redacted = text.replace(assignment, "[redacted]");
    if let Some((_, value)) = assignment.split_once('=') {
        if !value.is_empty() {
            redacted = redacted.replace(value, "[redacted]");
        }
    }
    redacted
}
