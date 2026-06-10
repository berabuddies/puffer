//! Workspace-scoped daemon client helpers for `puffer browser`.

use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use serde::Deserialize;
use serde_json::{json, Value};
use std::ffi::OsString;
use std::io::{BufRead, BufReader, ErrorKind, Read as _};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use tungstenite::{connect, Message};
use url::Url;
use uuid::Uuid;

use crate::daemon::Handshake;

const BROWSER_SESSION_ID_FILENAME: &str = "browser_cli_session_id";

/// Loads or creates the stable default CLI browser root session id for one workspace.
pub(crate) fn default_cli_session_id(paths: &ConfigPaths) -> Result<String> {
    ensure_workspace_dirs(paths)?;
    let path = browser_session_id_path(paths);
    if let Some(existing) = load_browser_session_id(&path)? {
        return Ok(existing);
    }
    let session_id = new_browser_session_id(&paths.workspace_root);
    std::fs::write(&path, format!("{session_id}\n"))
        .with_context(|| format!("write browser session id {}", path.display()))?;
    Ok(session_id)
}

fn browser_session_id_path(paths: &ConfigPaths) -> PathBuf {
    paths.workspace_config_dir.join(BROWSER_SESSION_ID_FILENAME)
}

fn load_browser_session_id(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(raw) => {
            let trimmed = raw.trim();
            Ok(is_valid_browser_session_id(trimmed).then(|| trimmed.to_string()))
        }
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
        Err(error) => {
            Err(error).with_context(|| format!("read browser session id {}", path.display()))
        }
    }
}

fn new_browser_session_id(workspace_root: &Path) -> String {
    let workspace_root = canonical_workspace_root(workspace_root);
    let slug = workspace_root
        .file_name()
        .and_then(|value| value.to_str())
        .map(sanitize_session_slug)
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "workspace".to_string());
    format!("cli-browser-{slug}-{}", Uuid::new_v4().simple())
}

fn is_valid_browser_session_id(value: &str) -> bool {
    !value.is_empty()
        && !value.contains(":browser:")
        && value
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

/// Ensures a matching daemon is available for the current workspace.
pub(crate) fn ensure_daemon(paths: &ConfigPaths) -> Result<Handshake> {
    ensure_workspace_dirs(paths)?;
    let probe_session_id = default_cli_session_id(paths)?;
    for path in handshake_paths(paths) {
        let Ok(handshake) = read_handshake(&path) else {
            continue;
        };
        if !handshake_matches_workspace(&handshake, &paths.workspace_root) {
            continue;
        }
        if send_daemon_request(
            &handshake,
            "browser_agent",
            json!({
                "action": "list",
                "sessionId": probe_session_id,
            }),
        )
        .is_ok()
        {
            return Ok(handshake);
        }
    }
    spawn_daemon(paths)
}

/// Sends one JSON-RPC request to the daemon and returns its result payload.
pub(crate) fn send_daemon_request(
    handshake: &Handshake,
    method: &str,
    params: Value,
) -> Result<Value> {
    let request_id = Uuid::new_v4().to_string();
    let endpoint = endpoint_with_token(&handshake.url, &handshake.token)?;
    let (mut socket, _) = connect(endpoint.as_str()).context("connect to Puffer daemon")?;
    let request = json!({
        "id": request_id,
        "method": method,
        "params": params,
    });
    socket
        .send(Message::Text(request.to_string().into()))
        .context("send browser request to daemon")?;
    loop {
        let message = socket.read().context("read browser response from daemon")?;
        let Message::Text(text) = message else {
            continue;
        };
        let response: DaemonResponse =
            serde_json::from_str(&text).context("decode browser daemon response")?;
        if response.id.as_deref() != Some(request_id.as_str()) {
            continue;
        }
        if let Some(error) = response.error {
            bail!("{}", error.message);
        }
        return response
            .result
            .ok_or_else(|| anyhow!("daemon returned no browser result"));
    }
}

fn spawn_daemon(paths: &ConfigPaths) -> Result<Handshake> {
    let binary = std::env::current_exe().context("resolve current puffer binary")?;
    let handshake_path = workspace_handshake_path(paths);
    let mut command = Command::new(&binary);
    command
        .current_dir(&paths.workspace_root)
        .arg("daemon")
        .arg("--bind")
        .arg("127.0.0.1:0")
        .arg("--handshake-file")
        .arg(&handshake_path)
        .arg("--print-handshake")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(resources_dir) = resolve_builtin_resources_dir(&binary) {
        command.env("PUFFER_BUILTIN_RESOURCES_DIR", resources_dir);
    }
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn `{}` daemon", binary.display()))?;
    let stdout = child.stdout.take().context("daemon stdout missing")?;
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .context("read daemon handshake line")?;
    let line = line.trim();
    if line.is_empty() {
        let mut stderr = String::new();
        if let Some(mut pipe) = child.stderr.take() {
            let _ = pipe.read_to_string(&mut stderr);
        }
        let _ = child.wait();
        if stderr.trim().is_empty() {
            bail!("daemon printed an empty handshake line");
        }
        bail!("daemon failed to start: {}", stderr.trim());
    }
    serde_json::from_str(line).context("decode daemon handshake")
}

fn read_handshake(path: &Path) -> Result<Handshake> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("read daemon handshake {}", path.display()))?;
    serde_json::from_str(&text).context("decode daemon handshake")
}

fn handshake_paths(paths: &ConfigPaths) -> Vec<PathBuf> {
    let workspace = workspace_handshake_path(paths);
    let user = paths.user_config_dir.join("daemon.handshake");
    if workspace == user {
        vec![workspace]
    } else {
        vec![workspace, user]
    }
}

fn workspace_handshake_path(paths: &ConfigPaths) -> PathBuf {
    paths.workspace_config_dir.join("daemon.handshake")
}

fn handshake_matches_workspace(handshake: &Handshake, workspace_root: &Path) -> bool {
    canonical_workspace_root(Path::new(&handshake.workspace_root))
        == canonical_workspace_root(workspace_root)
}

fn canonical_workspace_root(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

fn sanitize_session_slug(raw: &str) -> String {
    raw.chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect()
}

fn endpoint_with_token(raw: &str, token: &str) -> Result<String> {
    let mut url = Url::parse(raw).context("parse daemon URL")?;
    url.query_pairs_mut().append_pair("token", token);
    Ok(url.to_string())
}

fn resolve_builtin_resources_dir(binary: &Path) -> Option<OsString> {
    if std::env::var_os("PUFFER_BUILTIN_RESOURCES_DIR").is_some() {
        return None;
    }
    let parent = binary.parent()?;
    let candidates = [
        parent.join("resources"),
        parent.parent()?.join("resources"),
        parent.parent()?.parent()?.join("resources"),
    ];
    candidates
        .into_iter()
        .find(|path| path.is_dir())
        .map(|path| path.into_os_string())
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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn default_cli_session_id_is_stable_for_one_workspace() {
        let temp = tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        let left = default_cli_session_id(&paths).unwrap();
        let right = default_cli_session_id(&paths).unwrap();
        assert_eq!(left, right);
        assert!(left.starts_with("cli-browser-"));
        let stored = std::fs::read_to_string(browser_session_id_path(&paths)).unwrap();
        assert_eq!(stored.trim(), left);
    }

    #[test]
    fn invalid_browser_session_id_is_regenerated() {
        let temp = tempdir().unwrap();
        let paths = test_config_paths(temp.path());
        ensure_workspace_dirs(&paths).unwrap();
        let path = browser_session_id_path(&paths);
        std::fs::write(&path, "bad:browser:root\n").unwrap();

        let session_id = default_cli_session_id(&paths).unwrap();
        assert!(session_id.starts_with("cli-browser-"));
        assert_ne!(session_id, "bad:browser:root");
        assert_eq!(std::fs::read_to_string(path).unwrap().trim(), session_id);
    }

    #[test]
    fn handshake_path_prefers_workspace_scope() {
        let paths = ConfigPaths::discover("/tmp/puffer-browser-session");
        let candidates = handshake_paths(&paths);
        assert_eq!(
            candidates[0],
            paths.workspace_config_dir.join("daemon.handshake")
        );
    }

    fn test_config_paths(root: &Path) -> ConfigPaths {
        ConfigPaths {
            workspace_root: root.to_path_buf(),
            workspace_config_dir: root.join(".puffer"),
            user_config_dir: root.join("user-puffer"),
            builtin_resources_dir: root.join("resources"),
        }
    }
}
