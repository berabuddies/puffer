//! Runtime-local Browser tool client for the desktop daemon.

use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::path::Path;
use tungstenite::{connect, Message};
use url::Url;
use uuid::Uuid;

/// Executes the model-facing Browser tool against the local Puffer daemon.
pub(super) fn execute_browser_tool(
    cwd: &Path,
    current_session_id: &Uuid,
    input: Value,
) -> Result<String> {
    let mut params: BrowserToolInput =
        serde_json::from_value(input).context("invalid Browser tool input")?;
    normalize_session_id(&mut params, current_session_id);
    let payload = serde_json::to_value(params)?;
    let handshakes = read_handshake_candidates(cwd)?;
    let mut last_error = None;
    for handshake in &handshakes {
        match send_daemon_request(handshake, "browser_agent", payload.clone()) {
            Ok(response) => return Ok(serde_json::to_string_pretty(&response)?),
            Err(error) => last_error = Some(error),
        }
    }
    if let Some(error) = last_error {
        return Err(error);
    }
    bail!("Browser tool found no daemon handshakes")
}

fn normalize_session_id(params: &mut BrowserToolInput, current_session_id: &Uuid) {
    let use_current = params
        .session_id
        .as_deref()
        .map(|value| value.trim().is_empty() || value == "current")
        .unwrap_or(true);
    if use_current {
        params.session_id = Some(current_session_id.to_string());
    }
}

#[derive(Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct BrowserToolInput {
    action: String,
    #[serde(default, rename = "sessionId")]
    session_id: Option<String>,
    #[serde(default, rename = "tabId")]
    tab_id: Option<String>,
    #[serde(default)]
    label: Option<String>,
    #[serde(default)]
    url: Option<String>,
    #[serde(default, rename = "ref")]
    ref_id: Option<String>,
    #[serde(default)]
    value: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    key: Option<String>,
    #[serde(default)]
    script: Option<String>,
    #[serde(default)]
    direction: Option<String>,
    #[serde(default)]
    px: Option<u32>,
    #[serde(default)]
    width: Option<u32>,
    #[serde(default)]
    height: Option<u32>,
    #[serde(default)]
    activate: Option<bool>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DaemonHandshake {
    url: String,
    token: String,
    #[serde(default)]
    workspace_root: Option<String>,
}

#[derive(Debug, Deserialize)]
struct DaemonResponse {
    #[serde(default)]
    id: Option<String>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<DaemonError>,
}

#[derive(Debug, Deserialize)]
struct DaemonError {
    message: String,
}

fn read_handshake_candidates(cwd: &Path) -> Result<Vec<DaemonHandshake>> {
    let paths = ConfigPaths::discover(cwd);
    let workspace_root = canonical_workspace_root(&paths.workspace_root);
    let workspace_path = paths.workspace_config_dir.join("daemon.handshake");
    let user_path = paths.user_config_dir.join("daemon.handshake");

    let mut last_error = None;
    let workspace_handshake = match read_handshake_file(&workspace_path) {
        Ok(handshake) => handshake,
        Err(error) => {
            last_error = Some(error);
            None
        }
    };
    let user_handshake = match read_handshake_file(&user_path) {
        Ok(handshake) => handshake,
        Err(error) => {
            last_error = Some(error);
            None
        }
    };
    let handshakes = select_handshake_candidates_for_browser(
        workspace_handshake,
        user_handshake,
        &workspace_root,
    );
    if !handshakes.is_empty() {
        return Ok(handshakes);
    }

    if let Some(error) = last_error {
        return Err(error);
    }

    bail!(
        "Browser tool requires a running Puffer daemon at {} or {}",
        workspace_path.display(),
        user_path.display()
    );
}

fn read_handshake_file(path: &Path) -> Result<Option<DaemonHandshake>> {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(anyhow!(error).context(format!("read daemon handshake {}", path.display())));
        }
    };
    serde_json::from_str(&text)
        .map(Some)
        .context(format!("decode daemon handshake {}", path.display()))
}

#[cfg(test)]
fn select_handshake_for_browser(
    workspace_handshake: Option<DaemonHandshake>,
    user_handshake: Option<DaemonHandshake>,
    workspace_root: &Path,
) -> Option<DaemonHandshake> {
    select_handshake_candidates_for_browser(workspace_handshake, user_handshake, workspace_root)
        .into_iter()
        .next()
}

fn select_handshake_candidates_for_browser(
    workspace_handshake: Option<DaemonHandshake>,
    user_handshake: Option<DaemonHandshake>,
    workspace_root: &Path,
) -> Vec<DaemonHandshake> {
    let mut candidates = Vec::new();
    if let Some(handshake) = workspace_handshake
        .filter(|handshake| handshake_matches_workspace(handshake, workspace_root))
    {
        candidates.push(handshake);
    }
    if let Some(handshake) = user_handshake {
        candidates.push(handshake);
    }
    candidates
}

fn handshake_matches_workspace(handshake: &DaemonHandshake, workspace_root: &Path) -> bool {
    let workspace_root = canonical_workspace_root(workspace_root);
    handshake
        .workspace_root
        .as_deref()
        .map(Path::new)
        .map(canonical_workspace_root)
        .map(|candidate| candidate == workspace_root)
        .unwrap_or(true)
}

fn canonical_workspace_root(path: &Path) -> std::path::PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn send_daemon_request(handshake: &DaemonHandshake, method: &str, params: Value) -> Result<Value> {
    let request_id = Uuid::new_v4().to_string();
    let endpoint = endpoint_with_token(&handshake.url, &handshake.token)?;
    let (mut socket, _) = connect(endpoint.as_str()).context("connect to Puffer daemon")?;
    let request = json!({
        "id": request_id,
        "method": method,
        "params": params
    });
    socket
        .send(Message::Text(request.to_string().into()))
        .context("send Browser request to daemon")?;
    loop {
        let message = socket.read().context("read Browser response from daemon")?;
        let Message::Text(text) = message else {
            continue;
        };
        let response: DaemonResponse =
            serde_json::from_str(&text).context("decode Browser daemon response")?;
        if response.id.as_deref() != Some(request_id.as_str()) {
            continue;
        }
        if let Some(error) = response.error {
            bail!("{}", error.message);
        }
        return response
            .result
            .ok_or_else(|| anyhow!("daemon returned no Browser result"));
    }
}

fn endpoint_with_token(raw: &str, token: &str) -> Result<String> {
    let mut url = Url::parse(raw).context("parse daemon URL")?;
    url.query_pairs_mut().append_pair("token", token);
    Ok(url.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_missing_current_and_empty_session_ids() {
        let current = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
        let current_string = current.to_string();
        for value in [None, Some("current"), Some("  ")] {
            let mut params = BrowserToolInput {
                action: "list".to_string(),
                session_id: value.map(ToString::to_string),
                tab_id: None,
                label: None,
                url: None,
                ref_id: None,
                value: None,
                text: None,
                key: None,
                script: None,
                direction: None,
                px: None,
                width: None,
                height: None,
                activate: None,
            };
            normalize_session_id(&mut params, &current);
            assert_eq!(params.session_id.as_deref(), Some(current_string.as_str()));
        }
    }

    #[test]
    fn preserves_explicit_real_session_id() {
        let current = Uuid::parse_str("2ba8b01d-5e7a-46b6-b747-7bfe5f6fa36a").unwrap();
        let explicit = "b4f239fd-1493-4be7-a3a1-9e58fe612576";
        let mut params = BrowserToolInput {
            action: "list".to_string(),
            session_id: Some(explicit.to_string()),
            tab_id: None,
            label: None,
            url: None,
            ref_id: None,
            value: None,
            text: None,
            key: None,
            script: None,
            direction: None,
            px: None,
            width: None,
            height: None,
            activate: None,
        };
        normalize_session_id(&mut params, &current);
        assert_eq!(params.session_id.as_deref(), Some(explicit));
    }

    #[test]
    fn preserves_extended_browser_action_fields() {
        let params: BrowserToolInput = serde_json::from_value(serde_json::json!({
            "action": "select",
            "tabId": "t2",
            "ref": "@e5",
            "value": "New York",
            "direction": "down",
            "px": 480
        }))
        .unwrap();

        let roundtrip = serde_json::to_value(params).unwrap();
        assert_eq!(
            roundtrip.get("value").and_then(Value::as_str),
            Some("New York")
        );
        assert_eq!(
            roundtrip.get("direction").and_then(Value::as_str),
            Some("down")
        );
        assert_eq!(roundtrip.get("px").and_then(Value::as_u64), Some(480));
    }

    #[test]
    fn handshake_workspace_filter_accepts_matching_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let handshake = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "token".to_string(),
            workspace_root: Some(workspace.path().display().to_string()),
        };
        assert!(handshake_matches_workspace(&handshake, workspace.path()));
    }

    #[test]
    fn handshake_workspace_filter_rejects_other_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let handshake = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "token".to_string(),
            workspace_root: Some(other.path().display().to_string()),
        };
        assert!(!handshake_matches_workspace(&handshake, workspace.path()));
    }

    #[test]
    fn browser_handshake_prefers_matching_workspace_handshake() {
        let workspace = tempfile::tempdir().unwrap();
        let matching = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "workspace".to_string(),
            workspace_root: Some(workspace.path().display().to_string()),
        };
        let user = DaemonHandshake {
            url: "ws://127.0.0.1:2/ws".to_string(),
            token: "user".to_string(),
            workspace_root: Some("/other/workspace".to_string()),
        };

        let selected =
            select_handshake_for_browser(Some(matching), Some(user), workspace.path()).unwrap();
        assert_eq!(selected.token, "workspace");
    }

    #[test]
    fn browser_handshake_keeps_user_fallback_after_matching_workspace() {
        let workspace = tempfile::tempdir().unwrap();
        let matching = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "workspace".to_string(),
            workspace_root: Some(workspace.path().display().to_string()),
        };
        let user = DaemonHandshake {
            url: "ws://127.0.0.1:2/ws".to_string(),
            token: "user".to_string(),
            workspace_root: Some("/desktop/global".to_string()),
        };

        let selected =
            select_handshake_candidates_for_browser(Some(matching), Some(user), workspace.path());
        let tokens = selected
            .iter()
            .map(|handshake| handshake.token.as_str())
            .collect::<Vec<_>>();
        assert_eq!(tokens, vec!["workspace", "user"]);
    }

    #[test]
    fn browser_handshake_uses_user_handshake_as_global_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mismatched = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "workspace".to_string(),
            workspace_root: Some(other.path().display().to_string()),
        };
        let user = DaemonHandshake {
            url: "ws://127.0.0.1:2/ws".to_string(),
            token: "user".to_string(),
            workspace_root: Some("/desktop/global".to_string()),
        };

        let selected =
            select_handshake_for_browser(Some(mismatched), Some(user), workspace.path()).unwrap();
        assert_eq!(selected.token, "user");
    }

    #[test]
    fn browser_handshake_ignores_mismatched_workspace_without_user_fallback() {
        let workspace = tempfile::tempdir().unwrap();
        let other = tempfile::tempdir().unwrap();
        let mismatched = DaemonHandshake {
            url: "ws://127.0.0.1:1/ws".to_string(),
            token: "workspace".to_string(),
            workspace_root: Some(other.path().display().to_string()),
        };

        let selected = select_handshake_for_browser(Some(mismatched), None, workspace.path());
        assert!(selected.is_none());
    }
}
