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
