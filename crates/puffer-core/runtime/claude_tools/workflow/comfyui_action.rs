use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::time::Duration;

const COMFY_CLOUD_HOST: &str = "https://cloud.comfy.org";
const DEFAULT_TIMEOUT_MS: u64 = 15_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ComfyUiActionInput {
    action: String,
    #[serde(default)]
    choice: Option<Value>,
    #[serde(default)]
    api_key: Option<Value>,
}

struct ConfiguredApiKey {
    source: String,
    value: String,
}

/// Executes one ComfyUI action backed by verified Lambda Skill contracts.
pub fn execute_comfyui_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: ComfyUiActionInput =
        serde_json::from_value(input).context("invalid ComfyUiAction input")?;
    match parsed.action.as_str() {
        "configureCloud" => configure_cloud(parsed, probe_comfy_cloud),
        other => bail!("unsupported ComfyUiAction action `{other}`"),
    }
}

fn configure_cloud(
    input: ComfyUiActionInput,
    probe: impl FnOnce(&str, &str) -> Result<()>,
) -> Result<String> {
    let choice = input
        .choice
        .as_ref()
        .context("ComfyUiAction configureCloud requires choice")?;
    validate_cloud_choice(choice)?;
    let api_key = input
        .api_key
        .as_ref()
        .context("ComfyUiAction configureCloud requires apiKey")?;
    let configured = configured_api_key(api_key)?;
    probe(COMFY_CLOUD_HOST, &configured.value)?;
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "host": COMFY_CLOUD_HOST,
        "server_up": true,
        "api_key_source": configured.source,
        "credentials_redacted": true,
    }))?)
}

fn validate_cloud_choice(value: &Value) -> Result<()> {
    let text = collect_string_values(value).join(" ").to_ascii_lowercase();
    if text.contains("cloud") && !text.contains("local only") {
        return Ok(());
    }
    bail!("ComfyUiAction configureCloud requires a cloud setup choice")
}

fn configured_api_key(value: &Value) -> Result<ConfiguredApiKey> {
    let Some(object) = value.as_object() else {
        bail!("ComfyUiAction apiKey must be the configured-source report, not a raw key");
    };
    if object.get("configured").and_then(Value::as_bool) != Some(true) {
        bail!("ComfyUiAction apiKey report must have configured=true");
    }
    let sources = object
        .get("sources")
        .and_then(Value::as_array)
        .context("ComfyUiAction apiKey report must include sources")?;
    let sources = sources
        .iter()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|source| !source.is_empty())
        .collect::<Vec<_>>();
    if sources.is_empty() {
        bail!("ComfyUiAction apiKey report must include at least one source");
    }
    for source in sources {
        if let Some(value) = resolve_configured_key_source(source)? {
            return Ok(ConfiguredApiKey {
                source: source.to_string(),
                value,
            });
        }
    }
    bail!("COMFY_CLOUD_API_KEY is not available to the running Puffer process")
}

fn resolve_configured_key_source(source: &str) -> Result<Option<String>> {
    if source == "env:COMFY_CLOUD_API_KEY" {
        return Ok(std::env::var("COMFY_CLOUD_API_KEY")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty()));
    }
    let Some((path, key)) = source.rsplit_once(':') else {
        return Ok(None);
    };
    if key != "COMFY_CLOUD_API_KEY" {
        return Ok(None);
    }
    dotenv_value(&expand_home(path), key)
}

fn dotenv_value(path: &Path, key: &str) -> Result<Option<String>> {
    let Ok(text) = std::fs::read_to_string(path) else {
        return Ok(None);
    };
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once('=') else {
            continue;
        };
        if name.trim() == key {
            let value = value
                .trim()
                .trim_matches('"')
                .trim_matches('\'')
                .to_string();
            if !value.is_empty() {
                return Ok(Some(value));
            }
        }
    }
    Ok(None)
}

fn expand_home(path: &str) -> PathBuf {
    if path == "~" {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home);
        }
    }
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

fn collect_string_values(value: &Value) -> Vec<String> {
    match value {
        Value::String(text) => vec![text.clone()],
        Value::Array(items) => items.iter().flat_map(collect_string_values).collect(),
        Value::Object(object) => object.values().flat_map(collect_string_values).collect(),
        other => vec![other.to_string()],
    }
}

fn probe_comfy_cloud(host: &str, api_key: &str) -> Result<()> {
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Comfy Cloud HTTP client")?;
    let response = client
        .get(host)
        .header("X-API-Key", api_key)
        .send()
        .context("connect to Comfy Cloud")?;
    if !response.status().is_success() {
        bail!(
            "Comfy Cloud host check failed with status {}",
            response.status()
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};

    fn env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn configure_cloud_rejects_raw_api_key() {
        let input = ComfyUiActionInput {
            action: "configureCloud".to_string(),
            choice: Some(serde_json::json!("Cloud")),
            api_key: Some(serde_json::json!("comfyui-secret")),
        };
        let error = configure_cloud(input, |_, _| Ok(())).unwrap_err();
        assert!(error.to_string().contains("not a raw key"));
    }

    #[test]
    fn configure_cloud_requires_cloud_choice() {
        let input = ComfyUiActionInput {
            action: "configureCloud".to_string(),
            choice: Some(serde_json::json!("Local")),
            api_key: Some(
                serde_json::json!({"configured": true, "sources": ["env:COMFY_CLOUD_API_KEY"]}),
            ),
        };
        let error = configure_cloud(input, |_, _| Ok(())).unwrap_err();
        assert!(error.to_string().contains("cloud setup choice"));
    }

    #[test]
    fn configure_cloud_uses_configured_source_without_leaking_key() {
        let _guard = env_lock().lock().unwrap();
        std::env::set_var("COMFY_CLOUD_API_KEY", "comfyui-private");
        let input = ComfyUiActionInput {
            action: "configureCloud".to_string(),
            choice: Some(serde_json::json!({"answers": {"mode": "Comfy Cloud"}})),
            api_key: Some(serde_json::json!({
                "configured": true,
                "sources": ["env:COMFY_CLOUD_API_KEY"]
            })),
        };
        let output = configure_cloud(input, |host, key| {
            assert_eq!(host, COMFY_CLOUD_HOST);
            assert_eq!(key, "comfyui-private");
            Ok(())
        })
        .unwrap();
        std::env::remove_var("COMFY_CLOUD_API_KEY");
        assert!(!output.contains("comfyui-private"));
        assert!(output.contains(COMFY_CLOUD_HOST));
        assert!(output.contains("credentials_redacted"));
    }
}
