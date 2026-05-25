use anyhow::{bail, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    connection_workflow_trigger_supported, ConnectionRecord, ConnectorTemplate,
    SubscriberManifestRoots,
};
use puffer_workflow::{
    validate_workflow, AgentFlowPipeline, PipelineNode, TriggerSpec, WorkflowDefinition,
    WorkflowStore,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

/// Creates a disabled starter workflow from the terminal command surface.
pub(super) fn create_workflow(paths: &ConfigPaths, args: &str) -> Result<String> {
    let mut parts = args.split_whitespace();
    let slug_hint = parts.next().unwrap_or("workflow-draft");
    let trigger_hint = parts.next();
    if parts.next().is_some() {
        bail!("Usage: /workflows new [slug] [connection-slug]");
    }

    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let existing = store
        .list()?
        .into_iter()
        .map(|workflow| workflow.slug)
        .collect::<BTreeSet<_>>();
    let slug = unique_slug(&normalize_slug(slug_hint), &existing);
    let trigger = trigger_hint
        .map(connection_trigger)
        .unwrap_or_else(|| default_trigger(paths));
    let definition = starter_definition(paths, &slug, trigger);
    validate_workflow(&definition)?;
    let created = store.upsert(definition)?;

    let mut out = String::new();
    let _ = writeln!(out, "Created disabled workflow draft.");
    let _ = writeln!(out, "slug: {}", created.slug);
    let _ = writeln!(out, "trigger: {}", trigger_label(&created.trigger));
    let _ = writeln!(out, "nodes: {}", created.pipeline.nodes.len());
    let _ = writeln!(
        out,
        "next: edit it in Corbina Pipelines, then resume it when ready."
    );
    Ok(out)
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

fn default_trigger(paths: &ConfigPaths) -> TriggerSpec {
    trigger_ready_connection(paths)
        .map(|connection| connection_trigger(&connection.slug))
        .unwrap_or_else(|| TriggerSpec::Subscription {
            source_topic: "workspace.task.created".to_string(),
            pattern: Some(".*".to_string()),
            classify_prompt: None,
        })
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

fn connection_trigger(connection_slug: &str) -> TriggerSpec {
    TriggerSpec::Connection {
        connection_slug: connection_slug.to_string(),
        filter: None,
        pattern: Some(".*".to_string()),
        classify_prompt: None,
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
        TriggerSpec::Subscription { source_topic, .. } => format!("subscription:{source_topic}"),
        TriggerSpec::Connection {
            connection_slug, ..
        } => format!("connection:{connection_slug}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
