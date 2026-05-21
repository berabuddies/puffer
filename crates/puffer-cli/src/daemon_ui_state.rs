use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopPinState {
    #[serde(default)]
    pub(crate) pinned_agent_ids: Vec<String>,
    #[serde(default)]
    pub(crate) pinned_workspace_paths: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopFileTabsState {
    #[serde(default)]
    pub(crate) tabs: Vec<DesktopFileTab>,
    #[serde(default)]
    pub(crate) active_path: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopSessionRouting {
    #[serde(default)]
    pub(crate) provider_id: Option<String>,
    #[serde(default)]
    pub(crate) model_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct DesktopFileTab {
    pub(crate) path: String,
    #[serde(default)]
    pub(crate) pinned: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DesktopState {
    #[serde(default)]
    pinned_agent_ids: Vec<String>,
    #[serde(default)]
    pinned_workspace_paths: Vec<String>,
    #[serde(default)]
    file_tabs_by_session: BTreeMap<String, DesktopFileTabsState>,
    #[serde(default)]
    session_routing_by_session: BTreeMap<String, DesktopSessionRouting>,
}

/// Loads daemon-persisted desktop pin state from the user config directory.
pub(crate) fn load_pin_state(user_config_dir: &Path) -> Result<DesktopPinState> {
    let state = load_desktop_state(user_config_dir)?;
    Ok(normalize_pin_state(DesktopPinState {
        pinned_agent_ids: state.pinned_agent_ids,
        pinned_workspace_paths: state.pinned_workspace_paths,
    }))
}

/// Persists one agent or workspace pin and returns the updated state.
pub(crate) fn set_pin_state(
    user_config_dir: &Path,
    kind: &str,
    id: &str,
    pinned: bool,
) -> Result<DesktopPinState> {
    let mut state = load_pin_state(user_config_dir)?;
    match kind {
        "agent" => set_membership(&mut state.pinned_agent_ids, id, pinned),
        "workspace" => set_membership(&mut state.pinned_workspace_paths, id, pinned),
        other => anyhow::bail!("unsupported pin kind `{other}`"),
    }
    let mut desktop = load_desktop_state(user_config_dir)?;
    desktop.pinned_agent_ids = state.pinned_agent_ids.clone();
    desktop.pinned_workspace_paths = state.pinned_workspace_paths.clone();
    save_desktop_state(user_config_dir, &desktop)?;
    Ok(state)
}

/// Loads persisted Files tab state for one agent session.
pub(crate) fn load_file_tabs_state(
    user_config_dir: &Path,
    session_id: &str,
) -> Result<DesktopFileTabsState> {
    let state = load_desktop_state(user_config_dir)?;
    Ok(normalize_file_tabs_state(
        state
            .file_tabs_by_session
            .get(session_id.trim())
            .cloned()
            .unwrap_or_default(),
    ))
}

/// Persists Files tab state for one agent session.
pub(crate) fn set_file_tabs_state(
    user_config_dir: &Path,
    session_id: &str,
    file_tabs: DesktopFileTabsState,
) -> Result<DesktopFileTabsState> {
    let trimmed = session_id.trim();
    let mut state = load_desktop_state(user_config_dir)?;
    let normalized = normalize_file_tabs_state(file_tabs);
    if trimmed.is_empty() || normalized.tabs.is_empty() {
        state.file_tabs_by_session.remove(trimmed);
    } else {
        state
            .file_tabs_by_session
            .insert(trimmed.to_string(), normalized.clone());
    }
    save_desktop_state(user_config_dir, &state)?;
    Ok(normalized)
}

/// Loads persisted provider/model routing for one agent session.
pub(crate) fn load_session_routing_state(
    user_config_dir: &Path,
    session_id: &str,
) -> Result<DesktopSessionRouting> {
    let state = load_desktop_state(user_config_dir)?;
    Ok(normalize_session_routing(
        state
            .session_routing_by_session
            .get(session_id.trim())
            .cloned()
            .unwrap_or_default(),
    ))
}

/// Persists provider/model routing for one agent session.
pub(crate) fn set_session_routing_state(
    user_config_dir: &Path,
    session_id: &str,
    routing: DesktopSessionRouting,
) -> Result<DesktopSessionRouting> {
    let trimmed = session_id.trim();
    let mut state = load_desktop_state(user_config_dir)?;
    let normalized = normalize_session_routing(routing);
    if trimmed.is_empty() || (normalized.provider_id.is_none() && normalized.model_id.is_none()) {
        state.session_routing_by_session.remove(trimmed);
    } else {
        state
            .session_routing_by_session
            .insert(trimmed.to_string(), normalized.clone());
    }
    save_desktop_state(user_config_dir, &state)?;
    Ok(normalized)
}

fn load_desktop_state(user_config_dir: &Path) -> Result<DesktopState> {
    let path = pin_state_path(user_config_dir);
    if !path.exists() {
        return Ok(DesktopState::default());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("failed to read {}", path.display()))?;
    let state: DesktopState = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", path.display()))?;
    Ok(normalize_desktop_state(state))
}

fn save_desktop_state(user_config_dir: &Path, state: &DesktopState) -> Result<()> {
    let path = pin_state_path(user_config_dir);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let raw = serde_json::to_string_pretty(&normalize_desktop_state(state.clone()))?;
    std::fs::write(&path, raw).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(())
}

fn pin_state_path(user_config_dir: &Path) -> std::path::PathBuf {
    user_config_dir.join("desktop_state.json")
}

fn set_membership(values: &mut Vec<String>, id: &str, pinned: bool) {
    let trimmed = id.trim();
    if trimmed.is_empty() {
        return;
    }
    values.retain(|value| value != trimmed);
    if pinned {
        values.insert(0, trimmed.to_string());
    }
}

fn normalize_desktop_state(mut state: DesktopState) -> DesktopState {
    dedupe_nonempty(&mut state.pinned_agent_ids);
    dedupe_nonempty(&mut state.pinned_workspace_paths);
    state.file_tabs_by_session.retain(|session_id, tabs| {
        *tabs = normalize_file_tabs_state(tabs.clone());
        !session_id.trim().is_empty() && !tabs.tabs.is_empty()
    });
    state
        .session_routing_by_session
        .retain(|session_id, routing| {
            *routing = normalize_session_routing(routing.clone());
            !session_id.trim().is_empty()
                && (routing.provider_id.is_some() || routing.model_id.is_some())
        });
    state
}

fn normalize_pin_state(mut state: DesktopPinState) -> DesktopPinState {
    dedupe_nonempty(&mut state.pinned_agent_ids);
    dedupe_nonempty(&mut state.pinned_workspace_paths);
    state
}

fn normalize_file_tabs_state(mut state: DesktopFileTabsState) -> DesktopFileTabsState {
    let mut seen = std::collections::BTreeSet::new();
    state.tabs.retain(|tab| {
        let trimmed = tab.path.trim();
        !trimmed.is_empty() && seen.insert(trimmed.to_string())
    });
    for tab in &mut state.tabs {
        tab.path = tab.path.trim().to_string();
    }
    if let Some(active) = &state.active_path {
        let trimmed = active.trim();
        state.active_path = state
            .tabs
            .iter()
            .any(|tab| tab.path == trimmed)
            .then(|| trimmed.to_string());
    }
    if state.active_path.is_none() {
        state.active_path = state.tabs.first().map(|tab| tab.path.clone());
    }
    state
}

fn normalize_session_routing(mut state: DesktopSessionRouting) -> DesktopSessionRouting {
    state.provider_id = state
        .provider_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    state.model_id = state
        .model_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string);
    state
}

fn dedupe_nonempty(values: &mut Vec<String>) {
    let mut seen = std::collections::BTreeSet::new();
    values.retain(|value| {
        let trimmed = value.trim();
        !trimmed.is_empty() && seen.insert(trimmed.to_string())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_tabs_preserve_pin_state() {
        let dir = tempfile::tempdir().unwrap();
        set_pin_state(dir.path(), "agent", "session-a", true).unwrap();

        set_file_tabs_state(
            dir.path(),
            "session-a",
            DesktopFileTabsState {
                tabs: vec![DesktopFileTab {
                    path: "/tmp/a.rs".to_string(),
                    pinned: true,
                }],
                active_path: Some("/tmp/a.rs".to_string()),
            },
        )
        .unwrap();

        let pins = load_pin_state(dir.path()).unwrap();
        assert_eq!(pins.pinned_agent_ids, vec!["session-a"]);
    }

    #[test]
    fn pin_state_preserves_file_tabs() {
        let dir = tempfile::tempdir().unwrap();
        set_file_tabs_state(
            dir.path(),
            "session-a",
            DesktopFileTabsState {
                tabs: vec![DesktopFileTab {
                    path: "/tmp/a.rs".to_string(),
                    pinned: false,
                }],
                active_path: Some("/tmp/a.rs".to_string()),
            },
        )
        .unwrap();

        set_pin_state(dir.path(), "workspace", "/tmp/project", true).unwrap();

        let tabs = load_file_tabs_state(dir.path(), "session-a").unwrap();
        assert_eq!(tabs.tabs.len(), 1);
        assert_eq!(tabs.tabs[0].path, "/tmp/a.rs");
        assert_eq!(tabs.active_path.as_deref(), Some("/tmp/a.rs"));
    }

    #[test]
    fn file_tabs_normalize_duplicates_and_active_path() {
        let state = normalize_file_tabs_state(DesktopFileTabsState {
            tabs: vec![
                DesktopFileTab {
                    path: " /tmp/a.rs ".to_string(),
                    pinned: false,
                },
                DesktopFileTab {
                    path: "/tmp/a.rs".to_string(),
                    pinned: true,
                },
                DesktopFileTab {
                    path: "/tmp/b.rs".to_string(),
                    pinned: true,
                },
            ],
            active_path: Some("/tmp/missing.rs".to_string()),
        });

        assert_eq!(state.tabs.len(), 2);
        assert_eq!(state.tabs[0].path, "/tmp/a.rs");
        assert_eq!(state.active_path.as_deref(), Some("/tmp/a.rs"));
    }

    #[test]
    fn session_routing_preserves_provider_and_model() {
        let dir = tempfile::tempdir().unwrap();

        set_session_routing_state(
            dir.path(),
            "session-a",
            DesktopSessionRouting {
                provider_id: Some(" anthropic ".to_string()),
                model_id: Some(" claude-sonnet-4-5 ".to_string()),
            },
        )
        .unwrap();

        let routing = load_session_routing_state(dir.path(), "session-a").unwrap();
        assert_eq!(routing.provider_id.as_deref(), Some("anthropic"));
        assert_eq!(routing.model_id.as_deref(), Some("claude-sonnet-4-5"));
    }
}
