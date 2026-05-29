//! `Canvas` workflow tool — render an agent-produced typed spec into a
//! self-contained HTML result page and open it.
//!
//! The agent supplies only the SPEC (semantic data: title, counts, findings,
//! tables). The HTML template embeds the design system (tokens, layout, dark +
//! light), so every canvas is consistent rather than free-form HTML. The page
//! is written under `<cwd>/.puffer/canvas/` and opened best-effort.

use crate::AppState;
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

const TEMPLATE: &str = include_str!("canvas_template.html");

/// Render a canvas spec into a self-contained HTML document.
///
/// `bridge` (when present) lets canvas node actions continue the conversation:
/// the page calls back to the local daemon's `run_agent_turn` with the action's
/// bundled context, so a click starts a new agent turn — no prompt typing. It is
/// `{ url, token, sessionId }`; `null` when no daemon is reachable (CLI mode).
pub fn render_canvas(spec: &Value, bridge: Option<&Value>) -> String {
    let title = spec
        .get("title")
        .and_then(Value::as_str)
        .unwrap_or("Canvas");
    let spec_json = serde_json::to_string(spec).unwrap_or_else(|_| "{}".to_string());
    let bridge_json = bridge
        .map(|b| serde_json::to_string(b).unwrap_or_else(|_| "null".into()))
        .unwrap_or_else(|| "null".into());
    TEMPLATE
        .replace("__TITLE__", &escape_html(title))
        .replace("__SPEC__", &spec_json)
        .replace("__BRIDGE__", &bridge_json)
}

/// Resolve a daemon callback bridge from the on-disk handshake, if a daemon is
/// running. Returns `{ url, token, sessionId }`. Security note: this embeds the
/// daemon token into the generated HTML so a browser page can call back; the
/// file is written owner-only (0600). Corbina-native embedding (no token on
/// disk) is the hardened follow-up.
fn resolve_bridge(session_id: &str) -> Option<Value> {
    let home = std::env::var_os("HOME")?;
    let handshake = Path::new(&home).join(".puffer").join("daemon.handshake");
    let raw = std::fs::read_to_string(&handshake).ok()?;
    let line = raw.lines().next()?;
    let hs: Value = serde_json::from_str(line).ok()?;
    let url = hs.get("url").and_then(Value::as_str)?;
    let token = hs.get("token").and_then(Value::as_str)?;
    Some(json!({ "url": url, "token": token, "sessionId": session_id }))
}

fn escape_html(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Best-effort open of a file in the OS default app. Never fails the tool.
fn open_in_os(path: &Path) -> bool {
    #[cfg(target_os = "macos")]
    let prog = "open";
    #[cfg(target_os = "linux")]
    let prog = "xdg-open";
    #[cfg(target_os = "windows")]
    let prog = "explorer";
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    let prog = "open";
    std::process::Command::new(prog)
        .arg(path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .is_ok()
}

/// Executes the `Canvas` workflow tool. `input` is the canvas spec itself.
pub fn execute_canvas(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = state;
    if !input.is_object() {
        anyhow::bail!("Canvas input must be a JSON object (the canvas spec)");
    }
    let node_count = input
        .get("body")
        .and_then(Value::as_array)
        .map(|n| n.len())
        .unwrap_or(0);
    let bridge = resolve_bridge(&state.session.id.to_string());
    let interactive = bridge.is_some();
    let html = render_canvas(&input, bridge.as_ref());

    let dir = cwd.join(".puffer").join("canvas");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("failed to create canvas dir {}", dir.display()))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let path = dir.join(format!("canvas-{stamp}.html"));
    std::fs::write(&path, &html)
        .with_context(|| format!("failed to write canvas {}", path.display()))?;
    // Owner-only: the page may embed the daemon token for action callbacks.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    let opened = open_in_os(&path);

    Ok(serde_json::to_string_pretty(&json!({
        "status": "rendered",
        "path": path.display().to_string(),
        "nodes": node_count,
        "interactive": interactive,
        "opened": opened,
        "note": if opened {
            "Canvas rendered and opened in the default browser."
        } else {
            "Canvas rendered; open the file path in a browser to view it."
        }
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use serde_json::json;
    use uuid::Uuid;

    fn temp_state(cwd: std::path::PathBuf) -> AppState {
        let session = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        AppState::new(PufferConfig::default(), cwd, session)
    }

    #[test]
    fn render_embeds_title_and_spec() {
        let spec = json!({
            "title": "Demo <Review>",
            "body": [{ "type": "finding", "severity": "high", "title": "Issue" }]
        });
        let html = render_canvas(&spec, None);
        assert!(
            html.contains("Demo &lt;Review&gt;"),
            "title escaped into <title>"
        );
        assert!(html.contains("\"finding\""), "spec JSON embedded");
        assert!(html.contains("application/json"), "spec script tag present");
        // the design-system renderer (component registry) is present
        assert!(html.contains("const COMP="), "component registry embedded");
        // no daemon bridge -> actions degrade
        assert!(
            html.contains("=null") || html.contains("= null") || html.contains(":null"),
            "bridge is null without a daemon"
        );
    }

    #[test]
    fn render_embeds_bridge_for_actions() {
        let spec = json!({ "title": "T", "body": [] });
        let bridge =
            json!({ "url": "ws://127.0.0.1:5555/ws", "token": "tk", "sessionId": "sess-1" });
        let html = render_canvas(&spec, Some(&bridge));
        assert!(
            html.contains("ws://127.0.0.1:5555/ws"),
            "daemon url embedded for callback"
        );
        assert!(
            html.contains("sess-1"),
            "session id embedded so actions continue the turn"
        );
    }

    #[test]
    fn execute_writes_html_file() {
        let tempdir = tempfile::tempdir().unwrap();
        let cwd = tempdir.path().to_path_buf();
        let mut state = temp_state(cwd.clone());
        let spec = json!({
            "title": "T",
            "body": [
                { "type": "metrics", "items": [{ "value": "1", "label": "x" }] },
                { "type": "section", "title": "S", "children": [
                    { "type": "finding", "severity": "high", "title": "y" }
                ]}
            ]
        });
        let out = execute_canvas(&mut state, &cwd, spec).unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(parsed["status"], "rendered");
        assert_eq!(parsed["nodes"], 2);
        let path = parsed["path"].as_str().unwrap();
        assert!(std::path::Path::new(path).exists(), "html file written");
        let body = std::fs::read_to_string(path).unwrap();
        assert!(body.contains("<!doctype html>"));
    }
}
