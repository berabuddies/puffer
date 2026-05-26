use super::secret_value;
use crate::AppState;
use anyhow::{bail, Context, Result};
use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::process::Command;
use std::sync::OnceLock;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ModalActionInput {
    action: String,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    value: Option<Value>,
    #[serde(default)]
    gpu: Option<String>,
    #[serde(default)]
    image: Option<String>,
    #[serde(default)]
    schedule: Option<String>,
}

/// Executes a Modal CLI operation backed by verified Lambda Skill contracts.
pub fn execute_modal_action(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: ModalActionInput =
        serde_json::from_value(input).context("invalid ModalAction input")?;
    match parsed.action.as_str() {
        "secretCreate" => secret_create(state, cwd, parsed),
        "defineFunction" => define_function(cwd, parsed),
        "defineClass" => define_class(cwd, parsed),
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

fn define_function(cwd: &Path, input: ModalActionInput) -> Result<String> {
    let gpu = validate_gpu(&required_string(input.gpu, "gpu")?)?;
    let image = validate_modal_expr(
        &required_string(input.image, "image")?,
        "image",
        &["modal.Image."],
    )?;
    let schedule = validate_modal_expr(
        &required_string(input.schedule, "schedule")?,
        "schedule",
        &["modal.Cron(", "modal.Period("],
    )?;
    let function_name = generated_name("puffer_modal_function");
    let body = format!(
        r#"import modal

app = modal.App("puffer-verified-modal")
image = {image}

@app.function(
    gpu={gpu_literal},
    image=image,
    schedule={schedule},
    container_idle_timeout=300,
    allow_concurrent_inputs=10,
    memory=32768,
    cpu=4,
    timeout=3600,
    retries=3,
    concurrency_limit=10,
)
def {function_name}():
    return {{"ok": True}}
"#,
        gpu_literal = serde_json::to_string(&gpu)?
    );
    let path = write_modal_definition(cwd, &body)?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "app_defined": true,
        "kind": "function",
        "name": function_name,
        "path": path.display().to_string(),
    }))?)
}

fn define_class(cwd: &Path, input: ModalActionInput) -> Result<String> {
    let gpu = validate_gpu(&required_string(input.gpu, "gpu")?)?;
    let image = validate_modal_expr(
        &required_string(input.image, "image")?,
        "image",
        &["modal.Image."],
    )?;
    let class_name = generated_name("PufferModalModel");
    let body = format!(
        r#"import modal

app = modal.App("puffer-verified-modal")
image = {image}

@app.cls(gpu={gpu_literal}, image=image)
class {class_name}:
    @modal.enter()
    def load(self):
        pass

    @modal.method()
    def predict(self, value=None):
        return value
"#,
        gpu_literal = serde_json::to_string(&gpu)?
    );
    let path = write_modal_definition(cwd, &body)?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "app_defined": true,
        "kind": "class",
        "name": class_name,
        "path": path.display().to_string(),
    }))?)
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

fn generated_name(prefix: &str) -> String {
    let suffix = Uuid::new_v4().simple().to_string();
    format!("{prefix}_{}", &suffix[..8])
}

fn write_modal_definition(cwd: &Path, body: &str) -> Result<std::path::PathBuf> {
    let dir = cwd.join(".puffer/generated/modal");
    fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create Modal definition dir {}", dir.display()))?;
    let path = dir.join(format!("{}.py", Uuid::new_v4().simple()));
    fs::write(&path, body).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path)
}

fn validate_gpu(value: &str) -> Result<String> {
    validate_code_atom(value, "gpu", 128)?;
    static GPU: OnceLock<Regex> = OnceLock::new();
    let regex =
        GPU.get_or_init(|| Regex::new(r#"^[A-Za-z0-9][A-Za-z0-9:_+.,\-/ ]{0,127}$"#).unwrap());
    if regex.is_match(value) {
        return Ok(value.to_string());
    }
    bail!("ModalAction gpu value contains unsupported characters")
}

fn validate_modal_expr(value: &str, name: &str, prefixes: &[&str]) -> Result<String> {
    validate_code_atom(value, name, 2_000)?;
    if !prefixes.iter().any(|prefix| value.starts_with(prefix)) {
        bail!(
            "ModalAction {name} must begin with one of: {}",
            prefixes.join(", ")
        );
    }
    for forbidden in [
        "__",
        "import",
        "exec",
        "eval",
        "open(",
        "subprocess",
        "os.",
        "sys.",
        "builtins",
    ] {
        if value.contains(forbidden) {
            bail!("ModalAction {name} contains unsupported token `{forbidden}`");
        }
    }
    Ok(value.to_string())
}

fn validate_code_atom(value: &str, name: &str, max_chars: usize) -> Result<()> {
    if value.chars().count() > max_chars {
        bail!("ModalAction {name} is too long");
    }
    if value
        .chars()
        .any(|ch| ch.is_control() || matches!(ch, ';' | '`'))
    {
        bail!("ModalAction {name} must be a single expression");
    }
    Ok(())
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
