use crate::runtime::permission_prompt::{
    prompt_for_permission, PermissionPromptAction, PermissionPromptRequest,
};
use crate::runtime::secrets::register_masked_secret;
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_secrets::SecretVault;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestSecretInput {
    #[serde(default)]
    id: Option<String>,
    #[serde(default, alias = "name")]
    label: Option<String>,
    #[serde(default)]
    reason: Option<String>,
}

/// Requests access to one encrypted user secret and returns a masked placeholder.
pub fn execute_request_secret(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: RequestSecretInput =
        serde_json::from_value(input).context("invalid RequestSecret input")?;
    let selector = parsed
        .id
        .or(parsed.label)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("RequestSecret requires `id` or `label`")?;
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir))?;
    let secret = vault.reveal(&selector)?;
    if !state.secret_access_state.allows(&secret.id) {
        match prompt_for_secret(&secret.id, &secret.label, parsed.reason.as_deref()) {
            PermissionPromptAction::AllowOnce => {}
            PermissionPromptAction::AllowSession => {
                state.secret_access_state.allow_secret(secret.id.clone());
            }
            PermissionPromptAction::AllowAllSession => {
                state.secret_access_state.allow_all();
            }
            PermissionPromptAction::Deny => bail!("permission denied by user"),
        }
    }
    let token = register_masked_secret(state, secret.value)?;
    Ok(serde_json::to_string_pretty(&json!({
        "secret": token,
        "id": secret.id,
        "label": secret.label,
    }))?)
}

fn prompt_for_secret(secret_id: &str, label: &str, reason: Option<&str>) -> PermissionPromptAction {
    let summary = format!("Agent requested encrypted secret `{label}` ({secret_id})");
    let reason = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("{value} Approve this secret only or all secrets for this session."))
        .unwrap_or_else(|| "Approve this secret only or all secrets for this session.".to_string());
    prompt_for_permission(PermissionPromptRequest {
        tool_id: "RequestSecret".to_string(),
        summary,
        reason: Some(reason),
        browser: None,
        review: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::with_permission_prompt_handler;
    use crate::AppState;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use puffer_config::PufferConfig;
    use puffer_secrets::SecretUpsert;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_state(cwd: &Path) -> AppState {
        AppState::new(
            PufferConfig::default(),
            cwd.to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: cwd.to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    #[test]
    fn request_secret_returns_masked_token_only() {
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("PUFFER_SECRET_STORE_KEY", BASE64.encode([9u8; 32]));
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Demo".to_string(),
                value: "raw-secret".to_string(),
                username: None,
                origin: None,
                source: "manual".to_string(),
            })
            .unwrap();
        let output = with_permission_prompt_handler(
            |_| PermissionPromptAction::AllowOnce,
            || execute_request_secret(&mut state, dir.path(), json!({"label": "Demo"})),
        )
        .unwrap();
        assert!(output.contains("PUFFER_SECRET_"));
        assert!(!output.contains("raw-secret"));
        std::env::remove_var("PUFFER_SECRET_STORE_KEY");
    }
}
