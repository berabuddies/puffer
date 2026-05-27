use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use serde_json::Value;

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
    manager.refresh_connection_consumers()?;
    super::handle_workflow_list(paths)
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
