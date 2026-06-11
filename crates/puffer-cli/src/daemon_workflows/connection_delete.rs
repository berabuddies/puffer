use anyhow::{bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    ActionSpec, ConnectionRecord, SubscriptionManager, WorkflowBindingSpec,
};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Deletes one connector connection and returns the refreshed snapshot.
pub(crate) fn handle_workflow_connection_delete(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let slug = params
        .get("slug")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("missing slug")?;
    let manager = subscription_manager()?;
    let connections = manager.connection_store().list();
    let connection = connections
        .iter()
        .find(|connection| connection.slug == slug)
        .cloned();
    if let Some(connection) = connection.as_ref() {
        clear_external_auth_for_connection(connection, &connections)?;
        cleanup_wechat_container(connection);
    }
    manager.connection_store().delete(slug)?;
    delete_monitor_for_connection(paths, manager.as_ref(), slug)?;
    manager.refresh_connection_consumers()?;
    super::handle_workflow_list(paths)
}

fn clear_external_auth_for_connection(
    connection: &ConnectionRecord,
    connections: &[ConnectionRecord],
) -> Result<()> {
    for command in external_auth_clear_commands(connection, connections) {
        run_external_auth_clear_command(&command)?;
    }
    Ok(())
}

fn external_auth_clear_commands(
    connection: &ConnectionRecord,
    connections: &[ConnectionRecord],
) -> Vec<Vec<String>> {
    if !is_lark_connection(connection)
        || connections
            .iter()
            .any(|other| other.slug != connection.slug && is_lark_connection(other))
    {
        return Vec::new();
    }
    let bin = env_or("LARK_CLI_BIN", "lark-cli");
    vec![
        vec![bin.clone(), "auth".to_string(), "logout".to_string()],
        vec![bin, "config".to_string(), "remove".to_string()],
    ]
}

fn is_lark_connection(connection: &ConnectionRecord) -> bool {
    connection.connector_slug.starts_with("lark-")
}

/// Best-effort FULL teardown of a WeChat connection (container + data + state).
///
/// Each WeChat connection owns its own container (`puffer-wechat-<slug>`), so
/// deleting the connection removes the container (`stop` then `rm` — the
/// `--restart unless-stopped` policy could otherwise resurrect a bare `rm -f`),
/// its named data volume (the login + chat data at `/config`), and the
/// per-instance state files. Delete is a full reset: no account data is left
/// behind and a later re-add starts fresh (new QR scan). Errors are logged.
fn cleanup_wechat_container(connection: &ConnectionRecord) {
    if !connection.connector_slug.starts_with("wechat-") {
        return;
    }
    // Drive the SAME runtime the instance was created on (Docker or Apple
    // `container`) — resolved exactly as the connector does — so deleting a
    // connection on the `container` runtime removes the real container/volume
    // rather than running `docker` commands that can't see it.
    let instance = crate::wechat_connector::WechatInstance::for_connection(&connection.slug);
    let bin = instance.bin();
    let container = instance.container_name();
    // Stop first (ignore "not running"), then remove and capture the result.
    // Both runtimes accept `stop -t` and `rm -f`.
    let _ = Command::new(bin)
        .args(["stop", "-t", "3", &container])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match Command::new(bin)
        .args(["rm", "-f", &container])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .output()
    {
        Ok(out) if !out.status.success() => {
            let err = String::from_utf8_lossy(&out.stderr);
            // "No such container" is fine; anything else is worth surfacing.
            if !err.contains("No such container") {
                eprintln!("wechat cleanup: failed to remove `{container}`: {}", err.trim());
            }
        }
        Err(error) => {
            eprintln!("wechat cleanup: could not run `{bin} rm` for `{container}` (runtime down?): {error}");
        }
        _ => {}
    }
    // Delete is a FULL wipe: remove the data volume too (the login session + all
    // chat data live at /config). So a later re-add starts fresh (new QR scan),
    // and no account data is left behind. (A plain stop/restart keeps the volume;
    // only deleting the connection removes it.) Volume name == container name.
    // Apple `container` uses `volume delete`; Docker uses `volume rm -f`.
    let mut volume_cmd = Command::new(bin);
    if instance.is_container() {
        volume_cmd.args(["volume", "delete", &container]);
    } else {
        volume_cmd.args(["volume", "rm", "-f", &container]);
    }
    let _ = volume_cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    // Remove per-instance state files (config/policy/seen/lock + the cached DB keys).
    if let Some(dir) = wechat_state_dir() {
        for suffix in ["json", "policy.json", "seen.json", "policy.lock", "ui.lock", "dbkey"] {
            let _ = std::fs::remove_file(dir.join(format!("{}.{suffix}", connection.slug)));
        }
    }
}

/// Resolves the connector's per-instance state dir (must match where the
/// connector writes: `WECHAT_STATE_DIR`, else `~/.puffer/wechat`).
fn wechat_state_dir() -> Option<PathBuf> {
    if let Ok(dir) = std::env::var("WECHAT_STATE_DIR") {
        if !dir.trim().is_empty() {
            return Some(PathBuf::from(dir));
        }
    }
    let home = std::env::var("PUFFER_HOME")
        .ok()
        .filter(|v| !v.trim().is_empty())
        .or_else(|| std::env::var("HOME").ok())?;
    Some(PathBuf::from(home).join(".puffer").join("wechat"))
}

fn run_external_auth_clear_command(command: &[String]) -> Result<()> {
    let Some((binary, args)) = command.split_first() else {
        return Ok(());
    };
    let output = Command::new(binary)
        .args(args)
        .stdin(Stdio::null())
        .output()
        .with_context(|| format!("run `{}`", format_command(command)))?;
    if output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let detail = if stderr.is_empty() { stdout } else { stderr };
    bail!(
        "external auth cleanup failed for `{}`: {}",
        format_command(command),
        if detail.is_empty() {
            "command exited unsuccessfully"
        } else {
            detail.as_str()
        }
    );
}

fn format_command(command: &[String]) -> String {
    command.join(" ")
}

fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

fn delete_monitor_for_connection(
    paths: &ConfigPaths,
    manager: &SubscriptionManager,
    connection_slug: &str,
) -> Result<()> {
    for binding_slug in
        monitor_binding_slugs_for_connection(manager.store().list(), connection_slug)
    {
        manager.store().delete(&binding_slug)?;
    }
    let memory_path = monitor_memory_path(paths, connection_slug);
    match fs::remove_file(&memory_path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => {
            return Err(error)
                .with_context(|| format!("failed to remove {}", memory_path.display()));
        }
    }
    Ok(())
}

fn monitor_binding_slugs_for_connection(
    bindings: Vec<WorkflowBindingSpec>,
    connection_slug: &str,
) -> Vec<String> {
    bindings
        .into_iter()
        .filter(|binding| binding.connection_slug == connection_slug && is_monitor_binding(binding))
        .map(|binding| binding.slug)
        .collect()
}

fn is_monitor_binding(binding: &WorkflowBindingSpec) -> bool {
    binding.slug == monitor_slug(&binding.connection_slug)
        || (matches!(&binding.action, ActionSpec::TriageAgent { .. })
            && binding.description.to_ascii_lowercase().contains("monitor"))
}

fn monitor_slug(connection_slug: &str) -> String {
    format!("monitor-{connection_slug}")
}

fn monitor_memory_path(paths: &ConfigPaths, connection_slug: &str) -> PathBuf {
    paths
        .workspace_config_dir
        .join("runtime")
        .join("monitors")
        .join(format!("{connection_slug}.md"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::WorkflowBindingStatus;
    use serde_json::json;

    #[test]
    fn delete_params_require_slug() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let error = handle_workflow_connection_delete(&paths, &json!({})).unwrap_err();

        assert!(error.to_string().contains("missing slug"));
    }

    #[test]
    fn delete_params_reject_blank_slug() {
        let tempdir = tempfile::tempdir().unwrap();
        let paths = ConfigPaths::discover(tempdir.path());

        let error = handle_workflow_connection_delete(&paths, &json!({"slug": "  "})).unwrap_err();

        assert!(error.to_string().contains("missing slug"));
    }

    #[test]
    fn monitor_binding_slugs_match_connection_monitors() {
        let slugs = monitor_binding_slugs_for_connection(
            vec![
                sample_binding("monitor-demo", "demo", true),
                sample_binding("custom-monitor", "demo", true),
                sample_binding("ordinary-demo", "demo", false),
                sample_binding("monitor-other", "other", true),
            ],
            "demo",
        );

        assert_eq!(slugs, vec!["monitor-demo", "custom-monitor"]);
    }

    #[test]
    fn lark_connection_delete_clears_lark_cli_auth_and_config() {
        let connection = ConnectionRecord::authenticated("lark-user", "lark-login", "Lark");

        let commands = external_auth_clear_commands(&connection, std::slice::from_ref(&connection));

        assert_eq!(
            commands,
            vec![
                vec![
                    "lark-cli".to_string(),
                    "auth".to_string(),
                    "logout".to_string()
                ],
                vec![
                    "lark-cli".to_string(),
                    "config".to_string(),
                    "remove".to_string()
                ]
            ]
        );
    }

    #[test]
    fn lark_connection_delete_keeps_lark_cli_auth_when_another_lark_connection_exists() {
        let login = ConnectionRecord::authenticated("lark-user", "lark-login", "Lark user");
        let bot = ConnectionRecord::authenticated("lark-bot", "lark-bot", "Lark bot");

        let commands = external_auth_clear_commands(&login, &[login.clone(), bot]);

        assert!(commands.is_empty());
    }

    #[test]
    fn non_lark_connection_delete_does_not_clear_external_auth() {
        let connection = ConnectionRecord::authenticated("gmail-browser", "gmail-browser", "Gmail");

        assert!(
            external_auth_clear_commands(&connection, std::slice::from_ref(&connection)).is_empty()
        );
    }

    #[cfg(unix)]
    #[test]
    fn external_auth_clear_runner_invokes_command() {
        use std::os::unix::fs::PermissionsExt;

        let tempdir = tempfile::tempdir().unwrap();
        let marker = tempdir.path().join("logout-args.txt");
        let script = tempdir.path().join("fake-lark-cli");
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$*\" > '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        let mut permissions = std::fs::metadata(&script).unwrap().permissions();
        permissions.set_mode(0o755);
        std::fs::set_permissions(&script, permissions).unwrap();

        run_external_auth_clear_command(&[
            script.to_string_lossy().into_owned(),
            "auth".to_string(),
            "logout".to_string(),
        ])
        .unwrap();

        assert_eq!(std::fs::read_to_string(marker).unwrap(), "auth logout\n");
    }

    fn sample_binding(slug: &str, connection_slug: &str, monitor: bool) -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: slug.to_string(),
            description: if monitor {
                "Monitor demo"
            } else {
                "Append demo"
            }
            .to_string(),
            connection_slug: connection_slug.to_string(),
            connector_slug: Some("demo".to_string()),
            status: WorkflowBindingStatus::Enabled,
            filter: None,
            ignore_filters: Vec::new(),
            contact_ids: Vec::new(),
            classify_prompt: None,
            classify_model: None,
            action: if monitor {
                ActionSpec::TriageAgent {
                    prompt: "triage".to_string(),
                    model: None,
                }
            } else {
                ActionSpec::RunWorkflow {
                    slug: "workflow".to_string(),
                }
            },
            created_at_ms: 0,
        }
    }
}
