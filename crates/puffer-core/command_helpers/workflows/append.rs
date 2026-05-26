use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connection_subscriber_manifest, connection_workflow_trigger_supported,
    connector_workflow_trigger_supported, ActionSpec, ConnectionRecord, ConnectorTemplate,
    FilterSpec, SubscriberManifestRoots, TaggedFilterSpec, WorkflowBindingSpec,
    WorkflowBindingStatus,
};
use serde_json::json;
use std::fmt::Write as _;

/// Creates an enabled file-append workflow binding from the terminal command surface.
pub(super) fn create_append_binding(paths: &ConfigPaths, args: &str) -> Result<String> {
    let request = parse_append_args(args)?;
    let manager = crate::subscription_manager()?;
    let selection = resolve_trigger(
        paths,
        manager.as_ref(),
        &request.connection_slug,
        request.connector_slug.as_deref(),
    )?;
    let binding = binding_from_request(&request, selection.connector_slug.clone())?;
    let enabled = binding.status == WorkflowBindingStatus::Enabled;
    let binding_slug = binding.slug.clone();
    let previous = manager.store().get(&binding_slug);
    manager.store().upsert(binding.clone())?;
    let setup = (|| -> Result<()> {
        if enabled {
            if let Some(connection) = selection.connection.as_ref() {
                ensure_workflow_subscriber_started(
                    manager.as_ref(),
                    paths,
                    connection,
                    &selection.template,
                )?;
            }
        }
        manager.refresh_connection_consumers()?;
        Ok(())
    })();
    if let Err(error) = setup {
        let rollback = if let Some(previous) = previous {
            manager.store().upsert(previous).map(|_| ())
        } else {
            manager.store().delete(&binding_slug)
        };
        if let Err(rollback_error) = rollback {
            bail!(
                "workflow binding `{binding_slug}` setup failed: {error:#}; rollback failed: {rollback_error:#}"
            );
        }
        manager.refresh_connection_consumers()?;
        return Err(error)
            .with_context(|| format!("workflow binding `{binding_slug}` setup failed"));
    }

    let mut out = String::new();
    let _ = writeln!(out, "Created append workflow binding.");
    let _ = writeln!(out, "slug: {}", binding.slug);
    let _ = writeln!(out, "connection: {}", binding.connection_slug);
    let _ = writeln!(out, "connector: {}", selection.connector_slug);
    let _ = writeln!(out, "filter: {}", filter_label(binding.filter.as_ref()));
    let _ = writeln!(out, "action: file_append path={}", request.path);
    let _ = writeln!(
        out,
        "status: {}",
        if enabled { "enabled" } else { "paused" }
    );
    if selection.connection.is_none() {
        let _ = writeln!(
            out,
            "connect: /connect {} {}",
            selection.connector_slug, binding.connection_slug
        );
        let _ = writeln!(
            out,
            "note: this binding uses a planned connection; run connect before events can flow."
        );
    }
    Ok(out)
}

#[derive(Debug, Clone)]
struct AppendRequest {
    connection_slug: String,
    path: String,
    pattern: Option<String>,
    connector_slug: Option<String>,
    slug: Option<String>,
    enabled: bool,
}

struct TriggerSelection {
    connection: Option<ConnectionRecord>,
    connector_slug: String,
    template: ConnectorTemplate,
}

fn parse_append_args(args: &str) -> Result<AppendRequest> {
    let tokens = shell_words::split(args.trim())
        .map_err(|error| anyhow!("Invalid /workflows append arguments: {error}"))?;
    let mut connector_slug = None;
    let mut slug = None;
    let mut enabled = true;
    let mut positional = Vec::new();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index].as_str() {
            "--connector" => {
                index += 1;
                let Some(value) = tokens.get(index) else {
                    bail!("--connector requires a connector slug");
                };
                connector_slug = Some(value.clone());
            }
            "--slug" => {
                index += 1;
                let Some(value) = tokens.get(index) else {
                    bail!("--slug requires a workflow binding slug");
                };
                slug = Some(value.clone());
            }
            "--paused" => enabled = false,
            "--enabled" => enabled = true,
            value if value.starts_with("--connector=") => {
                connector_slug = Some(value.trim_start_matches("--connector=").to_string());
            }
            value if value.starts_with("--slug=") => {
                slug = Some(value.trim_start_matches("--slug=").to_string());
            }
            value if value.starts_with("--") => bail!("unknown option `{value}`"),
            value => positional.push(value.to_string()),
        }
        index += 1;
    }
    if positional.len() < 2 || positional.len() > 3 {
        bail!(
            "Usage: /workflows append <connection-slug> <file-path> [pattern] [--connector <connector-slug>] [--paused]"
        );
    }
    Ok(AppendRequest {
        connection_slug: positional[0].clone(),
        path: positional[1].clone(),
        pattern: positional.get(2).cloned(),
        connector_slug,
        slug,
        enabled,
    })
}

fn resolve_trigger(
    paths: &ConfigPaths,
    manager: &puffer_subscriptions::SubscriptionManager,
    connection_slug: &str,
    connector_slug: Option<&str>,
) -> Result<TriggerSelection> {
    let roots = subscriber_manifest_roots(paths);
    if let Some(connection) = manager.connection_store().get(connection_slug) {
        if let Some(connector_slug) = connector_slug {
            if connector_slug != connection.connector_slug {
                bail!(
                    "connection `{}` uses connector `{}`, not `{connector_slug}`",
                    connection.slug,
                    connection.connector_slug
                );
            }
        }
        let template = manager
            .connector_store()
            .get(&connection.connector_slug)
            .ok_or_else(|| anyhow!("connector `{}` not found", connection.connector_slug))?;
        if !connection_workflow_trigger_supported(&roots, &connection, &template) {
            bail!(
                "connection `{connection_slug}` uses connector {} which cannot start workflow triggers",
                connection.connector_slug
            );
        }
        return Ok(TriggerSelection {
            connection: Some(connection.clone()),
            connector_slug: connection.connector_slug.clone(),
            template,
        });
    }

    let connector_slug = connector_slug
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .context("missing --connector for a planned workflow connection")?;
    let template = manager
        .connector_store()
        .get(connector_slug)
        .ok_or_else(|| anyhow!("connector `{connector_slug}` not found"))?;
    if !connector_workflow_trigger_supported(&roots, &template) {
        bail!("connector `{connector_slug}` cannot produce workflow trigger events");
    }
    Ok(TriggerSelection {
        connection: None,
        connector_slug: connector_slug.to_string(),
        template,
    })
}

fn binding_from_request(
    request: &AppendRequest,
    connector_slug: String,
) -> Result<WorkflowBindingSpec> {
    let connection_slug = request.connection_slug.trim();
    if connection_slug.is_empty() {
        bail!("connection slug must not be empty");
    }
    let path = request.path.trim();
    if path.is_empty() {
        bail!("file path must not be empty");
    }
    let action = serde_json::from_value::<ActionSpec>(json!({
        "type": "file_append",
        "path": path,
        "format": "text",
    }))
    .context("build file_append action")?;
    let slug = request
        .slug
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| default_append_slug(connection_slug, path));
    Ok(WorkflowBindingSpec {
        slug,
        description: format!("Append {connection_slug} messages to {path}"),
        connection_slug: connection_slug.to_string(),
        connector_slug: Some(connector_slug),
        status: if request.enabled {
            WorkflowBindingStatus::Enabled
        } else {
            WorkflowBindingStatus::Paused
        },
        filter: normalized_pattern(request.pattern.as_deref()).map(|pattern| {
            FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern,
                case_insensitive: true,
            })
        }),
        classify_prompt: None,
        classify_model: None,
        action,
        created_at_ms: puffer_subscriptions::now_ms(),
    })
}

fn normalized_pattern(pattern: Option<&str>) -> Option<String> {
    let pattern = pattern.map(str::trim).unwrap_or_default();
    if pattern.is_empty() || pattern == "*" || pattern == ".*" {
        None
    } else {
        Some(pattern.to_string())
    }
}

fn default_append_slug(connection_slug: &str, path: &str) -> String {
    let path_leaf = path
        .rsplit('/')
        .find(|part| !part.trim().is_empty())
        .unwrap_or("events");
    format!("append-{connection_slug}-{}", slug_fragment(path_leaf))
}

fn slug_fragment(value: &str) -> String {
    let mut slug = String::new();
    let mut last_was_dash = false;
    for ch in value.chars().flat_map(char::to_lowercase) {
        if ch.is_ascii_lowercase() || ch.is_ascii_digit() {
            slug.push(ch);
            last_was_dash = false;
        } else if !last_was_dash && !slug.is_empty() {
            slug.push('-');
            last_was_dash = true;
        }
        if slug.len() >= 48 {
            break;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "events".to_string()
    } else {
        slug
    }
}

fn filter_label(filter: Option<&FilterSpec>) -> String {
    match filter {
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { pattern, .. })) => pattern.clone(),
        _ => ".*".to_string(),
    }
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    )
}

fn connector_stream_supported(template: &ConnectorTemplate) -> bool {
    template.can_subscribe && template.command_argv().is_some()
}

fn ensure_workflow_subscriber_started(
    manager: &puffer_subscriptions::SubscriptionManager,
    paths: &ConfigPaths,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> Result<()> {
    if connector_stream_supported(template) {
        return Ok(());
    }
    if let Some(manifest) =
        connection_subscriber_manifest(&subscriber_manifest_roots(paths), connection, template)?
    {
        manager.start_subscriber(manifest)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_append_args_with_connector_and_quoted_pattern() {
        let request =
            parse_append_args(r#"telegram-user /tmp/hi "hello world" --connector telegram-login"#)
                .unwrap();

        assert_eq!(request.connection_slug, "telegram-user");
        assert_eq!(request.path, "/tmp/hi");
        assert_eq!(request.pattern.as_deref(), Some("hello world"));
        assert_eq!(request.connector_slug.as_deref(), Some("telegram-login"));
        assert!(request.enabled);
    }

    #[test]
    fn builds_file_append_binding_with_regex_filter() {
        let request = parse_append_args("telegram-user /tmp/hi hi --paused").unwrap();

        let binding = binding_from_request(&request, "telegram-login".to_string()).unwrap();

        assert_eq!(binding.slug, "append-telegram-user-hi");
        assert_eq!(binding.connection_slug, "telegram-user");
        assert_eq!(binding.connector_slug.as_deref(), Some("telegram-login"));
        assert_eq!(binding.status, WorkflowBindingStatus::Paused);
        assert!(matches!(
            binding.filter,
            Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                ref pattern,
                case_insensitive: true
            })) if pattern == "hi"
        ));
        assert!(matches!(
            binding.action,
            ActionSpec::FileAppend {
                ref path,
                ..
            } if path == "/tmp/hi"
        ));
    }

    #[test]
    fn wildcard_pattern_means_no_filter() {
        let request = parse_append_args("telegram-user /tmp/hi '.*'").unwrap();

        let binding = binding_from_request(&request, "telegram-login".to_string()).unwrap();

        assert!(binding.filter.is_none());
    }
}
