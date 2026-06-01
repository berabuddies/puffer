use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriptions::{ActionSpec, SubscriptionManager, WorkflowBindingSpec};
use serde_json::Value;
use std::fs;
use std::path::PathBuf;

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
    manager.connection_store().delete(slug)?;
    delete_monitor_for_connection(paths, manager.as_ref(), slug)?;
    manager.refresh_connection_consumers()?;
    super::handle_workflow_list(paths)
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
