use crate::runtime::permission_prompt::{
    prompt_for_permission, prompt_for_user_question, PermissionPromptAction,
    PermissionPromptRequest, UserQuestionPromptRequest, UserQuestionPromptResponse,
};
use crate::runtime::secrets::{expand_secret_placeholder_value, register_masked_secret};
use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_secrets::{SecretSummary, SecretUpsert, SecretVault};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;

#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RequestSecretInput {
    #[serde(default)]
    action: Option<String>,
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    query: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    origin: Option<String>,
    #[serde(default)]
    reason: Option<String>,
    #[serde(default)]
    prompt: Option<String>,
}

/// Searches, creates, or requests one encrypted user secret.
pub fn execute_request_secret(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed = parse_request_secret_input(input)?;
    let action = parsed
        .action
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_ascii_lowercase)
        .unwrap_or_else(|| "request".to_string());
    let paths = ConfigPaths::discover(cwd);
    ensure_workspace_dirs(&paths)?;
    let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir))?;
    match action.as_str() {
        "request" | "get" | "reveal" => request_secret(state, vault, parsed),
        "search" | "list" => search_secrets(vault, parsed),
        "create" => create_secret(state, vault, parsed),
        "collect" => collect_secret(state, vault, parsed),
        other => bail!("unsupported RequestSecret action `{other}`"),
    }
}

fn parse_request_secret_input(input: Value) -> Result<RequestSecretInput> {
    match input {
        Value::String(query) => {
            let query = query.trim().to_string();
            if query.is_empty() {
                bail!("RequestSecret string input must be a non-empty search query");
            }
            Ok(RequestSecretInput {
                action: Some("search".to_string()),
                query: Some(query),
                ..RequestSecretInput::default()
            })
        }
        value => {
            let mut parsed: RequestSecretInput =
                serde_json::from_value(value).context("invalid RequestSecret input")?;
            normalize_request_secret_label(&mut parsed);
            if parsed.action.as_deref().is_none_or(str::is_empty)
                && parsed.id.as_deref().is_none_or(str::is_empty)
                && parsed.label.as_deref().is_none_or(str::is_empty)
                && parsed
                    .query
                    .as_deref()
                    .is_some_and(|query| !query.trim().is_empty())
            {
                parsed.action = Some("search".to_string());
            }
            Ok(parsed)
        }
    }
}

fn normalize_request_secret_label(parsed: &mut RequestSecretInput) {
    let has_label = parsed
        .label
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty());
    if !has_label {
        if let Some(name) = parsed
            .name
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            parsed.label = Some(name.to_string());
        }
    }
}

fn request_secret(
    state: &mut AppState,
    vault: SecretVault,
    parsed: RequestSecretInput,
) -> Result<String> {
    let selector = parsed
        .id
        .or(parsed.label)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("RequestSecret requires `id` or `label`")?;
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
    // A stored value is usually plaintext — browser and 1Password imports save the
    // resolved password. Only a manually-added `op://...` reference is resolved to
    // its live value on demand, so that reference's plaintext never persists.
    let value = if puffer_secrets::is_op_reference(&secret.value) {
        puffer_secrets::resolve_op_reference(&secret.value)
            .with_context(|| format!("resolve 1Password reference for secret `{}`", secret.label))?
    } else {
        secret.value
    };
    let token = register_masked_secret(state, value)?;
    Ok(serde_json::to_string_pretty(&json!({
        "secret": token,
        "id": secret.id,
        "name": secret.label,
        "label": secret.label,
        "description": secret.description,
        "username": secret.username,
        "origin": secret.origin,
    }))?)
}

fn search_secrets(vault: SecretVault, parsed: RequestSecretInput) -> Result<String> {
    let query = request_secret_search_query(&parsed);
    let limit = parsed.limit.unwrap_or(20).clamp(1, 100);
    let secrets = vault
        .search(&query, limit)?
        .into_iter()
        .map(secret_summary_json)
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&json!({
        "secrets": secrets,
        "count": secrets.len(),
        "query": query,
    }))?)
}

fn request_secret_search_query(parsed: &RequestSecretInput) -> String {
    let mut parts = Vec::new();
    push_search_hint(&mut parts, parsed.query.as_deref());
    push_search_hint(&mut parts, parsed.label.as_deref());
    push_search_hint(&mut parts, parsed.description.as_deref());
    push_search_hint(&mut parts, parsed.username.as_deref());
    push_search_hint(&mut parts, parsed.origin.as_deref());
    parts.join(" ")
}

fn push_search_hint(parts: &mut Vec<String>, value: Option<&str>) {
    if let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) {
        parts.push(value.to_string());
    }
}

fn create_secret(
    state: &AppState,
    vault: SecretVault,
    parsed: RequestSecretInput,
) -> Result<String> {
    let label = parsed
        .label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("RequestSecret create requires `name` or `label`")?;
    let value = parsed
        .value
        .filter(|value| !value.trim().is_empty())
        .context("RequestSecret create requires non-empty `value`")?;
    let value = expand_secret_placeholder_value(state, &value).map_err(|_| {
        anyhow::anyhow!(
            "RequestSecret create requires `value` to be a single Puffer secret placeholder; use action=collect to ask the user for a new secret"
        )
    })?;
    match prompt_for_secret_create(&label, parsed.reason.as_deref()) {
        PermissionPromptAction::AllowOnce
        | PermissionPromptAction::AllowSession
        | PermissionPromptAction::AllowAllSession => {}
        PermissionPromptAction::Deny => bail!("permission denied by user"),
    }
    let summary = vault.put(SecretUpsert {
        id: parsed.id,
        label,
        description: parsed.description,
        value,
        username: parsed.username,
        origin: parsed.origin,
        source: "agent".to_string(),
    })?;
    Ok(serde_json::to_string_pretty(&json!({
        "secret": secret_summary_json(summary),
        "created": true,
    }))?)
}

fn collect_secret(
    state: &mut AppState,
    vault: SecretVault,
    parsed: RequestSecretInput,
) -> Result<String> {
    let label = parsed
        .label
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .context("RequestSecret collect requires `name` or `label`")?;
    let question = collect_secret_prompt(&label, parsed.prompt.as_deref());
    let response = prompt_for_user_question(UserQuestionPromptRequest {
        questions: json!([{
            "type": "input",
            "question": question,
            "header": "Secret",
            "secret": true
        }]),
        metadata: serde_json::Value::Null,
    })
    .context("RequestSecret collect requires an active user question prompt")?;
    let value = collect_secret_answer(&response, &question)?;
    let summary = vault.put(SecretUpsert {
        id: parsed.id,
        label,
        description: parsed.description,
        value: value.clone(),
        username: parsed.username,
        origin: parsed.origin,
        source: "agent".to_string(),
    })?;
    let token = register_masked_secret(state, value)?;
    Ok(serde_json::to_string_pretty(&json!({
        "secret": token,
        "created": true,
        "stored": secret_summary_json(summary),
    }))?)
}

fn collect_secret_prompt(label: &str, prompt: Option<&str>) -> String {
    let prompt = prompt
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| {
            format!("Enter the secret value to save as encrypted Puffer secret `{label}`.")
        });
    format!("{prompt}\n\nPuffer will save this value as encrypted secret `{label}` and return only a placeholder to the agent.")
}

fn collect_secret_answer(response: &UserQuestionPromptResponse, question: &str) -> Result<String> {
    if let Some(answer) = response.answers.get(question).and_then(Value::as_str) {
        let answer = answer.trim().to_string();
        if !answer.is_empty() {
            return Ok(answer);
        }
    }
    let string_answers = response
        .answers
        .values()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|answer| !answer.is_empty())
        .collect::<Vec<_>>();
    if let [answer] = string_answers.as_slice() {
        return Ok((*answer).to_string());
    }
    bail!("secret collection was canceled or returned no value")
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

fn prompt_for_secret_create(label: &str, reason: Option<&str>) -> PermissionPromptAction {
    let summary = format!("Agent wants to save encrypted secret `{label}`");
    let reason = reason
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(|value| format!("{value} Approve saving this secret to the encrypted vault."))
        .unwrap_or_else(|| "Approve saving this secret to the encrypted vault.".to_string());
    prompt_for_permission(PermissionPromptRequest {
        tool_id: "RequestSecret".to_string(),
        summary,
        reason: Some(reason),
        browser: None,
        review: None,
    })
}

fn secret_summary_json(summary: SecretSummary) -> Value {
    json!({
        "id": summary.id,
        "name": summary.label,
        "label": summary.label,
        "description": summary.description,
        "username": summary.username,
        "origin": summary.origin,
        "source": summary.source,
        "createdAtMs": summary.created_at_ms,
        "updatedAtMs": summary.updated_at_ms,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::runtime::{with_permission_prompt_handler, with_user_question_prompt_handler};
    use crate::AppState;
    use base64::engine::general_purpose::STANDARD as BASE64;
    use base64::Engine;
    use puffer_config::{set_puffer_home_override, PufferConfig, PufferHomeOverride};
    use puffer_secrets::SecretUpsert;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use std::sync::{Mutex, MutexGuard, OnceLock};
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

    struct SecretTestEnv {
        _lock: MutexGuard<'static, ()>,
        _home: PufferHomeOverride,
    }

    impl Drop for SecretTestEnv {
        fn drop(&mut self) {
            std::env::remove_var("PUFFER_SECRET_STORE_KEY");
        }
    }

    fn secret_env_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    fn secret_test_env(home: &Path) -> SecretTestEnv {
        let lock = secret_env_lock().lock().unwrap();
        std::env::set_var("PUFFER_SECRET_STORE_KEY", BASE64.encode([9u8; 32]));
        let home = set_puffer_home_override(home);
        SecretTestEnv {
            _lock: lock,
            _home: home,
        }
    }

    #[test]
    fn request_secret_returns_masked_token_only() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Demo".to_string(),
                description: None,
                value: "raw-secret".to_string(),
                username: Some("demo@example.com".to_string()),
                origin: Some("https://example.com/login".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();
        let output = with_permission_prompt_handler(
            |_| PermissionPromptAction::AllowOnce,
            || execute_request_secret(&mut state, dir.path(), json!({"label": "Demo"})),
        )
        .unwrap();
        assert!(output.contains("PUFFER_SECRET_"));
        assert!(output.contains("demo@example.com"));
        assert!(output.contains("https://example.com/login"));
        assert!(!output.contains("raw-secret"));
    }

    #[test]
    fn request_secret_search_returns_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Deploy token".to_string(),
                description: Some("production deploy token".to_string()),
                value: "raw-deploy-secret".to_string(),
                username: None,
                origin: Some("https://deploy.example".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();

        let output = execute_request_secret(
            &mut state,
            dir.path(),
            json!({"action": "search", "query": "production"}),
        )
        .unwrap();

        assert!(output.contains("Deploy token"));
        assert!(output.contains("production deploy token"));
        assert!(!output.contains("raw-deploy-secret"));
    }

    #[test]
    fn request_secret_search_accepts_name_and_label_payload() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "DoorDash login".to_string(),
                description: Some("DoorDash account password".to_string()),
                value: "raw-doordash-password".to_string(),
                username: Some("user@example.com".to_string()),
                origin: Some("https://www.doordash.com".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();

        let output = execute_request_secret(
            &mut state,
            dir.path(),
            json!({
                "action": "search",
                "id": "",
                "name": "",
                "label": "",
                "description": "DoorDash login credentials",
                "query": "DoorDash",
                "limit": 10,
                "value": "",
                "prompt": "",
                "username": "",
                "origin": "doordash.com",
                "reason": "Find saved DoorDash credentials to help log in."
            }),
        )
        .unwrap();

        assert!(output.contains("DoorDash login"));
        assert!(output.contains("DoorDash account password"));
        assert!(!output.contains("raw-doordash-password"));
    }

    #[test]
    fn request_secret_search_uses_origin_hint_for_generic_queries() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Chrome user@example.com @ www.doordash.com".to_string(),
                description: Some("saved Chrome credential".to_string()),
                value: "raw-doordash-password".to_string(),
                username: Some("user@example.com".to_string()),
                origin: Some("https://www.doordash.com".to_string()),
                source: "chrome".to_string(),
            })
            .unwrap();

        let output = execute_request_secret(
            &mut state,
            dir.path(),
            json!({
                "action": "search",
                "query": "login account password",
                "origin": "https://www.doordash.com",
                "limit": 10
            }),
        )
        .unwrap();

        assert!(output.contains("www.doordash.com"));
        assert!(!output.contains("raw-doordash-password"));
    }

    #[test]
    fn request_secret_string_input_searches_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Google password".to_string(),
                description: Some("test.user@example.com".to_string()),
                value: "raw-google-password".to_string(),
                username: Some("test.user@example.com".to_string()),
                origin: Some("https://accounts.google.com".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();

        let output = execute_request_secret(
            &mut state,
            dir.path(),
            json!("test.user@example.com google password"),
        )
        .unwrap();

        assert!(output.contains("Google password"));
        assert!(output.contains("test.user@example.com"));
        assert!(!output.contains("raw-google-password"));
        assert!(!output.contains("PUFFER_SECRET_"));
    }

    #[test]
    fn request_secret_query_without_action_searches_metadata_only() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let paths = ConfigPaths::discover(dir.path());
        ensure_workspace_dirs(&paths).unwrap();
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        vault
            .put(SecretUpsert {
                id: None,
                label: "Calendar password".to_string(),
                description: Some("google calendar login".to_string()),
                value: "raw-calendar-password".to_string(),
                username: None,
                origin: Some("https://calendar.google.com".to_string()),
                source: "manual".to_string(),
            })
            .unwrap();

        let output =
            execute_request_secret(&mut state, dir.path(), json!({"query": "calendar google"}))
                .unwrap();

        assert!(output.contains("Calendar password"));
        assert!(output.contains("google calendar login"));
        assert!(!output.contains("raw-calendar-password"));
        assert!(!output.contains("PUFFER_SECRET_"));
    }

    #[test]
    fn request_secret_create_saves_metadata_without_echoing_value() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let token = register_masked_secret(&state, "raw-pager-secret".to_string()).unwrap();

        let output = with_permission_prompt_handler(
            |_| PermissionPromptAction::AllowOnce,
            || {
                execute_request_secret(
                    &mut state,
                    dir.path(),
                    json!({
                        "action": "create",
                        "name": "Pager token",
                        "description": "PagerDuty API token",
                        "value": token
                    }),
                )
            },
        )
        .unwrap();

        assert!(output.contains("Pager token"));
        assert!(output.contains("PagerDuty API token"));
        assert!(!output.contains("raw-pager-secret"));
        let paths = ConfigPaths::discover(dir.path());
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        assert_eq!(
            vault.reveal("Pager token").unwrap().value,
            "raw-pager-secret"
        );
    }

    #[test]
    fn request_secret_create_rejects_raw_value() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());

        let error = execute_request_secret(
            &mut state,
            dir.path(),
            json!({
                "action": "create",
                "name": "Raw token",
                "value": "raw-secret"
            }),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires `value` to be a single Puffer secret placeholder"));
    }

    #[test]
    fn request_secret_create_rejects_compound_placeholder_value() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let token = register_masked_secret(&state, "raw-token".to_string()).unwrap();

        let error = execute_request_secret(
            &mut state,
            dir.path(),
            json!({
                "action": "create",
                "name": "Compound token",
                "value": format!("{token}-raw-suffix")
            }),
        )
        .unwrap_err();

        assert!(error
            .to_string()
            .contains("requires `value` to be a single Puffer secret placeholder"));
    }

    #[test]
    fn request_secret_collect_prompts_saves_and_returns_masked_token_only() {
        let dir = tempfile::tempdir().unwrap();
        let _secret_env = secret_test_env(dir.path());
        let mut state = temp_state(dir.path());
        let prompt = "Enter the Ridge checkout password to save in Puffer.";

        let output = with_user_question_prompt_handler(
            move |request| {
                assert_eq!(request.questions[0]["type"], "input");
                assert_eq!(request.questions[0]["secret"], true);
                let question = request.questions[0]["question"].as_str().unwrap();
                assert!(question.contains(prompt));
                assert!(question.contains("Puffer will save this value"));
                crate::runtime::UserQuestionPromptResponse {
                    answers: serde_json::Map::from_iter([(
                        question.to_string(),
                        json!("raw-collected-password"),
                    )]),
                    annotations: serde_json::Map::new(),
                }
            },
            || {
                execute_request_secret(
                    &mut state,
                    dir.path(),
                    json!({
                        "action": "collect",
                        "name": "Ridge checkout",
                        "description": "Checkout login credential",
                        "origin": "https://ridge.com",
                        "username": "user@example.com",
                        "prompt": prompt
                    }),
                )
            },
        )
        .unwrap();

        assert!(output.contains("PUFFER_SECRET_"));
        assert!(output.contains("Ridge checkout"));
        assert!(!output.contains("raw-collected-password"));
        let paths = ConfigPaths::discover(dir.path());
        let vault = SecretVault::open(SecretVault::default_path(&paths.user_config_dir)).unwrap();
        let stored = vault.reveal("Ridge checkout").unwrap();
        assert_eq!(stored.value, "raw-collected-password");
        let summary = vault
            .search("Ridge checkout", 10)
            .unwrap()
            .into_iter()
            .find(|summary| summary.label == "Ridge checkout")
            .unwrap();
        assert_eq!(summary.origin.as_deref(), Some("https://ridge.com"));
        assert_eq!(summary.username.as_deref(), Some("user@example.com"));
    }
}
