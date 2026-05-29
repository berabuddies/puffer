use anyhow::{anyhow, bail, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    builtin_connector_templates, connection_workflow_trigger_supported,
    connector_workflow_trigger_supported, suggested_connection_slug, ConnectionRecord,
    ConnectorTemplate, SubscriberManifestRoots,
};
use puffer_workflow::{
    validate_workflow, AgentFlowPipeline, PipelineNode, TriggerSpec, WorkflowDefinition,
    WorkflowStore,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Creates a disabled starter workflow from the terminal command surface.
pub(super) fn create_workflow(paths: &ConfigPaths, args: &str) -> Result<String> {
    let request = parse_create_args(args)?;

    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let existing = store
        .list()?
        .into_iter()
        .map(|workflow| workflow.slug)
        .collect::<BTreeSet<_>>();
    let slug = unique_slug(&normalize_slug(&request.slug_hint), &existing);
    let selection = trigger_selection(paths, &request)?;
    let definition = starter_definition(paths, &slug, selection.trigger);
    validate_workflow(&definition)?;
    let created = store.upsert(definition)?;

    let mut out = String::new();
    let _ = writeln!(out, "Created disabled workflow draft.");
    let _ = writeln!(out, "slug: {}", created.slug);
    let _ = writeln!(out, "trigger: {}", trigger_label(&created.trigger));
    if let Some(command) = selection.connect_command {
        let _ = writeln!(out, "connect: {command}");
        let _ = writeln!(
            out,
            "note: this draft uses a planned connection; run connect before resuming."
        );
    }
    let _ = writeln!(out, "nodes: {}", created.pipeline.nodes.len());
    let _ = writeln!(
        out,
        "next: edit it in the Workflows screen, then resume it when ready."
    );
    Ok(out)
}

struct CreateWorkflowRequest {
    slug_hint: String,
    connection_slug: Option<String>,
    pattern: Option<String>,
}

#[derive(Debug)]
struct TriggerSelection {
    trigger: TriggerSpec,
    connect_command: Option<String>,
}

fn parse_create_args(args: &str) -> Result<CreateWorkflowRequest> {
    let tokens = shell_words::split(args.trim())
        .map_err(|error| anyhow!("Invalid /workflows new arguments: {error}"))?;
    if tokens.len() > 3 {
        bail!("Usage: /workflows new [slug] [connection-slug] [pattern]");
    }
    Ok(CreateWorkflowRequest {
        slug_hint: tokens
            .first()
            .cloned()
            .unwrap_or_else(|| "workflow-draft".to_string()),
        connection_slug: tokens.get(1).cloned(),
        pattern: tokens.get(2).cloned(),
    })
}

fn starter_definition(paths: &ConfigPaths, slug: &str, trigger: TriggerSpec) -> WorkflowDefinition {
    WorkflowDefinition {
        schema: "puffer.workflow.v1".to_string(),
        slug: slug.to_string(),
        enabled: false,
        trigger,
        pipeline: AgentFlowPipeline {
            name: title_from_slug(slug),
            working_dir: Some(paths.workspace_root.display().to_string()),
            concurrency: Some(1),
            nodes: vec![PipelineNode {
                id: "codex-task".to_string(),
                node_type: Some("codex".to_string()),
                agent: Some("Codex implementer".to_string()),
                prompt: "Handle the incoming workflow event and summarize verification."
                    .to_string(),
                model: Some("gpt-5.4-codex".to_string()),
                tools: ["read", "edit", "bash", "mcp"]
                    .into_iter()
                    .map(str::to_string)
                    .collect(),
                env: BTreeMap::new(),
                depends_on: Vec::new(),
                extra: BTreeMap::new(),
            }],
            extra: BTreeMap::new(),
        },
    }
}

fn trigger_selection(
    paths: &ConfigPaths,
    request: &CreateWorkflowRequest,
) -> Result<TriggerSelection> {
    match request.connection_slug.as_deref() {
        Some(connection_slug) => {
            explicit_connection_trigger(paths, connection_slug, request.pattern.as_deref())
        }
        None => Ok(TriggerSelection {
            trigger: default_trigger(paths),
            connect_command: None,
        }),
    }
}

fn default_trigger(paths: &ConfigPaths) -> TriggerSpec {
    trigger_ready_connection(paths)
        .map(|connection| connection_trigger(&connection.slug, None))
        .unwrap_or_else(|| TriggerSpec::Subscription {
            source_topic: "workspace.task.created".to_string(),
            pattern: Some(".*".to_string()),
            classify_prompt: None,
        })
}

fn explicit_connection_trigger(
    paths: &ConfigPaths,
    connection_slug: &str,
    pattern: Option<&str>,
) -> Result<TriggerSelection> {
    let roots = SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    );
    match crate::subscription_manager() {
        Ok(manager) => {
            let connectors = manager.connector_store().list_with_builtins();
            let connections = manager.connection_store().list();
            resolve_connection_trigger_from_context(
                &roots,
                &connectors,
                &connections,
                connection_slug,
                pattern,
                false,
            )
        }
        Err(error) => {
            let connectors = builtin_connector_templates();
            resolve_connection_trigger_from_context(
                &roots,
                &connectors,
                &[],
                connection_slug,
                pattern,
                true,
            )
            .map_err(|_| {
                anyhow!(
                    "Connector runtime unavailable; cannot verify connection {connection_slug}: {error}"
                )
            })
        }
    }
}

fn resolve_connection_trigger_from_context(
    roots: &SubscriberManifestRoots,
    connectors: &[ConnectorTemplate],
    connections: &[ConnectionRecord],
    connection_slug: &str,
    pattern: Option<&str>,
    allow_catalog_planned: bool,
) -> Result<TriggerSelection> {
    if let Some(connection) = connections
        .iter()
        .find(|connection| connection.slug == connection_slug)
    {
        let Some(template) = connectors
            .iter()
            .find(|template| template.slug == connection.connector_slug)
        else {
            bail!(
                "Connection {connection_slug} uses unknown connector {}. Run /connect {} {} to repair it.",
                connection.connector_slug,
                connection.connector_slug,
                connection.slug
            );
        };
        if !trigger_supported(roots, connection, template) {
            bail!(
                "Connection {connection_slug} uses connector {} which cannot start workflow triggers. Run /workflows connections trigger-ready to pick a trigger-ready connection.",
                connection.connector_slug
            );
        }
        return Ok(TriggerSelection {
            trigger: connection_trigger(connection_slug, pattern),
            connect_command: None,
        });
    }

    if let Some(template) = connectors.iter().find(|template| {
        suggested_connection_slug(&template.slug) == connection_slug
            && (connector_workflow_trigger_supported(roots, template)
                || (allow_catalog_planned && template.can_subscribe))
    }) {
        return Ok(TriggerSelection {
            trigger: connection_trigger(connection_slug, pattern),
            connect_command: Some(format!("/connect {} {connection_slug}", template.slug)),
        });
    }

    bail!(
        "Connection {connection_slug} is not configured. Run /workflows connections trigger-ready, or connect it first with /connect <connector-slug> {connection_slug}."
    );
}

fn trigger_ready_connection(paths: &ConfigPaths) -> Option<ConnectionRecord> {
    let manager = crate::subscription_manager().ok()?;
    let roots = SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    );
    let connectors = manager.connector_store().list_with_builtins();
    let mut connections = manager.connection_store().list();
    connections.sort_by(|left, right| {
        right
            .has_consumer
            .cmp(&left.has_consumer)
            .then_with(|| left.slug.cmp(&right.slug))
    });
    connections.into_iter().find(|connection| {
        connectors
            .iter()
            .find(|template| template.slug == connection.connector_slug)
            .is_some_and(|template| trigger_supported(&roots, connection, template))
    })
}

fn trigger_supported(
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> bool {
    connection_workflow_trigger_supported(roots, connection, template)
}

fn connection_trigger(connection_slug: &str, pattern: Option<&str>) -> TriggerSpec {
    TriggerSpec::Connection {
        connection_slug: connection_slug.to_string(),
        filter: None,
        pattern: Some(trigger_pattern(pattern)),
        classify_prompt: None,
    }
}

fn trigger_pattern(pattern: Option<&str>) -> String {
    let pattern = pattern.unwrap_or(".*").trim();
    if pattern.is_empty() || pattern == "*" {
        ".*".to_string()
    } else {
        pattern.to_string()
    }
}

fn normalize_slug(value: &str) -> String {
    let mut slug = String::new();
    for ch in value.trim().chars() {
        if ch.is_ascii_alphanumeric() {
            slug.push(ch.to_ascii_lowercase());
        } else if !slug.ends_with('-') {
            slug.push('-');
        }
    }
    let slug = slug.trim_matches('-').to_string();
    if slug.is_empty() {
        "workflow-draft".to_string()
    } else {
        slug
    }
}

fn unique_slug(base: &str, existing: &BTreeSet<String>) -> String {
    if !existing.contains(base) {
        return base.to_string();
    }
    for index in 2.. {
        let candidate = format!("{base}-{index}");
        if !existing.contains(&candidate) {
            return candidate;
        }
    }
    unreachable!("unbounded suffix loop must return");
}

fn title_from_slug(slug: &str) -> String {
    slug.split('-')
        .filter(|part| !part.is_empty())
        .map(|part| {
            let mut chars = part.chars();
            match chars.next() {
                Some(first) => format!("{}{}", first.to_ascii_uppercase(), chars.as_str()),
                None => String::new(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

fn trigger_label(trigger: &TriggerSpec) -> String {
    match trigger {
        TriggerSpec::Cron { cron } => format!("cron:{cron}"),
        TriggerSpec::Subscription {
            source_topic,
            pattern,
            ..
        } => trigger_with_pattern("subscription", source_topic, pattern.as_deref()),
        TriggerSpec::Connection {
            connection_slug,
            pattern,
            ..
        } => trigger_with_pattern("connection", connection_slug, pattern.as_deref()),
    }
}

fn trigger_with_pattern(kind: &str, value: &str, pattern: Option<&str>) -> String {
    match pattern {
        Some(pattern) => format!("{kind}:{value} pattern={pattern}"),
        None => format!("{kind}:{value}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::ConnectionState;
    use serde_json::json;

    #[test]
    fn normalizes_and_suffixes_workflow_slugs() {
        let existing = BTreeSet::from_iter([
            "customer-triage".to_string(),
            "customer-triage-2".to_string(),
        ]);

        assert_eq!(normalize_slug("Customer Triage!"), "customer-triage");
        assert_eq!(
            unique_slug("customer-triage", &existing),
            "customer-triage-3"
        );
    }

    #[test]
    fn parses_quoted_create_pattern() {
        let parsed = parse_create_args(r#"hi-archive telegram-user "hello world""#).unwrap();

        assert_eq!(parsed.slug_hint, "hi-archive");
        assert_eq!(parsed.connection_slug.as_deref(), Some("telegram-user"));
        assert_eq!(parsed.pattern.as_deref(), Some("hello world"));
    }

    #[test]
    fn connection_trigger_defaults_to_regex_wildcard() {
        let trigger = connection_trigger("telegram-user", Some("*"));

        assert_eq!(
            trigger_label(&trigger),
            "connection:telegram-user pattern=.*"
        );
    }

    #[test]
    fn resolves_existing_connection_with_pattern() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let connectors = vec![trigger_template()];
        let connections = vec![connection_record("demo-main", "demo-chat")];

        let selection = resolve_connection_trigger_from_context(
            &roots,
            &connectors,
            &connections,
            "demo-main",
            Some("hi"),
            false,
        )
        .unwrap();

        assert!(selection.connect_command.is_none());
        assert_eq!(
            trigger_label(&selection.trigger),
            "connection:demo-main pattern=hi"
        );
    }

    #[test]
    fn resolves_planned_suggested_connection_with_connect_command() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let connectors = vec![trigger_template()];

        let selection = resolve_connection_trigger_from_context(
            &roots,
            &connectors,
            &[],
            "demo-chat",
            None,
            false,
        )
        .unwrap();

        assert_eq!(
            selection.connect_command.as_deref(),
            Some("/connect demo-chat demo-chat")
        );
        assert_eq!(
            trigger_label(&selection.trigger),
            "connection:demo-chat pattern=.*"
        );
    }

    #[test]
    fn rejects_setup_only_connection_trigger() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let mut connector = trigger_template();
        connector.can_subscribe = false;
        connector.command.clear();
        let connections = vec![connection_record("demo-main", "demo-chat")];

        let error = resolve_connection_trigger_from_context(
            &roots,
            &[connector],
            &connections,
            "demo-main",
            None,
            false,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("cannot start workflow triggers"));
    }

    #[test]
    fn rejects_unknown_connection_trigger() {
        let roots = SubscriberManifestRoots::new("/tmp/workspace", "/tmp/user", "/tmp/builtin");
        let connectors = vec![trigger_template()];

        let error = resolve_connection_trigger_from_context(
            &roots,
            &connectors,
            &[],
            "custom-main",
            None,
            false,
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("Connection custom-main is not configured"));
    }

    fn trigger_template() -> ConnectorTemplate {
        ConnectorTemplate {
            slug: "demo-chat".to_string(),
            description: "Demo chat".to_string(),
            skill: "demo".to_string(),
            binary: "demo".to_string(),
            command: vec!["true".to_string()],
            requires_auth: false,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: json!({}),
            actions: BTreeMap::new(),
        }
    }

    fn connection_record(slug: &str, connector_slug: &str) -> ConnectionRecord {
        ConnectionRecord {
            slug: slug.to_string(),
            connector_slug: connector_slug.to_string(),
            description: "Demo main channel".to_string(),
            state: ConnectionState::Authenticated,
            has_consumer: false,
            cursor: None,
            auth_failure_notified: false,
        }
    }
}
