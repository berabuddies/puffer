use crate::daemon_ui_state::{set_session_routing_state, DesktopSessionRouting};
use anyhow::Result;
use std::path::Path;

/// Persists the resolved provider/model for an explicit desktop turn route.
pub(crate) fn persist_explicit_turn_routing(
    user_config_dir: &Path,
    session_id: &str,
    explicit_routing: bool,
    current_provider: Option<&str>,
    current_model: Option<&str>,
) -> Result<bool> {
    if !explicit_routing {
        return Ok(false);
    }
    let provider_id = current_provider
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    let model_id = normalized_current_model(provider_id.as_deref(), current_model);
    if provider_id.is_none() && model_id.is_none() {
        return Ok(false);
    }
    set_session_routing_state(
        user_config_dir,
        session_id,
        DesktopSessionRouting {
            provider_id,
            model_id,
        },
    )?;
    Ok(true)
}

fn normalized_current_model(
    provider_id: Option<&str>,
    current_model: Option<&str>,
) -> Option<String> {
    let trimmed = current_model?.trim();
    if trimmed.is_empty() {
        return None;
    }
    let Some(provider_id) = provider_id else {
        return Some(trimmed.to_string());
    };
    let Some((prefix, model_id)) = trimmed.split_once('/') else {
        return Some(trimmed.to_string());
    };
    if prefix.trim() == provider_id && !model_id.trim().is_empty() {
        return Some(model_id.trim().to_string());
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::daemon_ui_state::load_session_routing_state;

    #[test]
    fn explicit_turn_route_persists_resolved_model_without_provider_prefix() {
        let dir = tempfile::tempdir().unwrap();

        let changed = persist_explicit_turn_routing(
            dir.path(),
            "session-a",
            true,
            Some("openai"),
            Some("openai/gpt-5.4"),
        )
        .unwrap();

        assert!(changed);
        let routing = load_session_routing_state(dir.path(), "session-a").unwrap();
        assert_eq!(routing.provider_id.as_deref(), Some("openai"));
        assert_eq!(routing.model_id.as_deref(), Some("gpt-5.4"));
    }

    #[test]
    fn non_routing_turn_does_not_overwrite_saved_route() {
        let dir = tempfile::tempdir().unwrap();
        set_session_routing_state(
            dir.path(),
            "session-a",
            DesktopSessionRouting {
                provider_id: Some("anthropic".to_string()),
                model_id: Some("claude-sonnet-4-5".to_string()),
            },
        )
        .unwrap();

        let changed = persist_explicit_turn_routing(
            dir.path(),
            "session-a",
            false,
            Some("openai"),
            Some("openai/gpt-5.4"),
        )
        .unwrap();

        assert!(!changed);
        let routing = load_session_routing_state(dir.path(), "session-a").unwrap();
        assert_eq!(routing.provider_id.as_deref(), Some("anthropic"));
        assert_eq!(routing.model_id.as_deref(), Some("claude-sonnet-4-5"));
    }
}
