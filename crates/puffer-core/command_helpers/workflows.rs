use crate::{subscription_manager, AppState};
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    builtin_connector_templates, connection_workflow_trigger_supported,
    connector_workflow_trigger_supported, suggested_connection_slug, ConnectionRecord,
    ConnectorTemplate, SubscriberManifestRoots,
};
use puffer_workflow::{TriggerSpec, WorkflowDefinition, WorkflowRun, WorkflowStore};
use std::fmt::Write as _;

/// Renders a terminal-friendly workflow, connection, and connector summary.
pub(crate) fn handle_workflows_command(state: &AppState, args: &str) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let workflows = store.list()?;
    let runs = store.list_runs()?;
    let connector_context = connector_context();
    let roots = subscriber_manifest_roots(&paths);
    let mut parts = args.split_whitespace();
    let mode = parts.next().unwrap_or_default();
    let query = parts.collect::<Vec<_>>().join(" ");

    let mut out = String::new();
    match mode {
        "" | "show" | "list" => {
            write_header(&mut out, &paths, &workflows, &runs, &connector_context);
            write_workflows(&mut out, &workflows, &runs);
            write_connections(&mut out, &connector_context, &roots, "");
            write_connectors(&mut out, &connector_context, &roots, false, "");
        }
        "connections" | "connection" => {
            write_header(&mut out, &paths, &workflows, &runs, &connector_context);
            write_connections(&mut out, &connector_context, &roots, &query);
        }
        "connectors" | "connector" => {
            write_header(&mut out, &paths, &workflows, &runs, &connector_context);
            write_connectors(&mut out, &connector_context, &roots, true, &query);
        }
        "runs" | "run" => {
            write_header(&mut out, &paths, &workflows, &runs, &connector_context);
            write_runs(&mut out, &runs, &query);
        }
        _ => {
            out.push_str(
                "Usage: /workflows [list|connections|connectors|runs] [query]\n\
                 Tip: use /connect to add a connector connection, then select it as a workflow trigger.",
            );
        }
    }
    Ok(out)
}

struct ConnectorContext {
    connectors: Vec<ConnectorTemplate>,
    connections: Vec<ConnectionRecord>,
    error: Option<String>,
}

fn connector_context() -> ConnectorContext {
    match subscription_manager() {
        Ok(manager) => {
            let error = manager
                .refresh_connection_consumers()
                .err()
                .map(|error| error.to_string());
            ConnectorContext {
                connectors: manager.connector_store().list_with_builtins(),
                connections: manager.connection_store().list(),
                error,
            }
        }
        Err(error) => ConnectorContext {
            connectors: builtin_connector_templates(),
            connections: Vec::new(),
            error: Some(error.to_string()),
        },
    }
}

fn write_header(
    out: &mut String,
    paths: &ConfigPaths,
    workflows: &[WorkflowDefinition],
    runs: &[WorkflowRun],
    context: &ConnectorContext,
) {
    let _ = writeln!(out, "Workflow dashboard");
    let _ = writeln!(out, "workspace={}", paths.workspace_root.display());
    let _ = writeln!(
        out,
        "workflows={} runs={} connections={} connectors={}",
        workflows.len(),
        runs.len(),
        context.connections.len(),
        context.connectors.len()
    );
    if let Some(error) = &context.error {
        let _ = writeln!(out, "connector_runtime={}", first_line(error));
    }
}

fn write_workflows(out: &mut String, workflows: &[WorkflowDefinition], runs: &[WorkflowRun]) {
    out.push_str("\nWorkflows\n");
    if workflows.is_empty() {
        out.push_str("- none registered\n");
        return;
    }
    for workflow in workflows {
        let latest = runs
            .iter()
            .filter(|run| run.workflow_slug == workflow.slug)
            .max_by_key(|run| run.idx);
        let latest_text = latest
            .map(|run| format!("latest=#{} {:?}", run.idx, run.status))
            .unwrap_or_else(|| "latest=none".to_string());
        let _ = writeln!(
            out,
            "- {} [{}] trigger={} nodes={} {}",
            workflow.slug,
            if workflow.enabled {
                "enabled"
            } else {
                "paused"
            },
            trigger_label(&workflow.trigger),
            workflow.pipeline.nodes.len(),
            latest_text
        );
    }
}

fn write_connections(
    out: &mut String,
    context: &ConnectorContext,
    roots: &SubscriberManifestRoots,
    query: &str,
) {
    out.push_str("\nConnections\n");
    let connections = context
        .connections
        .iter()
        .filter(|connection| {
            matches_query(
                query,
                [
                    connection.slug.as_str(),
                    connection.connector_slug.as_str(),
                    connection.description.as_str(),
                ],
            )
        })
        .collect::<Vec<_>>();
    if connections.is_empty() && context.connections.is_empty() {
        out.push_str("- none configured; run /connect <connector-slug> <connection-name>\n");
        return;
    }
    if connections.is_empty() {
        out.push_str("- no matching connections\n");
        return;
    }
    for connection in connections {
        let consumer = if connection.has_consumer {
            "consumer=active"
        } else {
            "consumer=idle"
        };
        let trigger = if connection_trigger_supported(context, roots, connection) {
            "trigger=ready"
        } else {
            "trigger=unavailable"
        };
        let _ = writeln!(
            out,
            "- {} connector={} state={:?} {} {} {}",
            connection.slug,
            connection.connector_slug,
            connection.state,
            consumer,
            trigger,
            connection.description
        );
    }
}

fn write_connectors(
    out: &mut String,
    context: &ConnectorContext,
    roots: &SubscriberManifestRoots,
    include_all: bool,
    query: &str,
) {
    out.push_str("\nConnectors\n");
    let connectors = context
        .connectors
        .iter()
        .filter(|connector| include_all || connector_workflow_trigger_supported(roots, connector))
        .filter(|connector| {
            let action_slugs = connector_action_slugs(connector);
            matches_query(
                query,
                action_slugs.into_iter().chain([
                    connector.slug.as_str(),
                    connector.description.as_str(),
                    connector.skill.as_str(),
                ]),
            )
        })
        .collect::<Vec<_>>();
    if connectors.is_empty() && context.connectors.is_empty() {
        out.push_str("- none available\n");
        return;
    }
    if connectors.is_empty() {
        out.push_str("- no matching connectors\n");
        return;
    }
    for connector in connectors {
        let mut capabilities = Vec::new();
        if connector.requires_auth {
            capabilities.push("auth");
        }
        if connector.can_subscribe {
            capabilities.push("events");
        }
        if connector.can_proxy_agent {
            capabilities.push("proxy");
        }
        if connector_workflow_trigger_supported(roots, connector) {
            capabilities.push("trigger");
        }
        if !connector.actions.is_empty() {
            capabilities.push("actions");
        }
        let action_summary = connector_action_summary(connector, query)
            .map(|summary| format!(" actions={summary}"))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} [{}] {}{} connect=/connect {} {}",
            connector.slug,
            capabilities.join(","),
            connector.description,
            action_summary,
            connector.slug,
            suggested_connection_slug(&connector.slug)
        );
    }
}

fn connector_action_slugs(connector: &ConnectorTemplate) -> Vec<&str> {
    connector.actions.keys().map(String::as_str).collect()
}

fn connector_action_summary(connector: &ConnectorTemplate, query: &str) -> Option<String> {
    let slugs = connector_action_slugs(connector);
    if slugs.is_empty() {
        return None;
    }
    let matching = matching_action_slugs(&slugs, query);
    let visible_source = if matching.is_empty() {
        slugs.as_slice()
    } else {
        matching.as_slice()
    };
    let visible = visible_source.iter().take(3).copied().collect::<Vec<_>>();
    let hidden = slugs.len().saturating_sub(visible.len());
    let mut summary = visible.join(",");
    if hidden > 0 {
        let _ = write!(summary, ",+{hidden}");
    }
    Some(summary)
}

fn matching_action_slugs<'a>(slugs: &[&'a str], query: &str) -> Vec<&'a str> {
    let terms = search_terms(query);
    if terms.is_empty() {
        return Vec::new();
    }
    slugs
        .iter()
        .copied()
        .filter(|slug| {
            let normalized = slug.to_ascii_lowercase();
            terms.iter().all(|term| normalized.contains(term))
        })
        .collect()
}

fn connection_trigger_supported(
    context: &ConnectorContext,
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
) -> bool {
    context
        .connectors
        .iter()
        .find(|template| template.slug == connection.connector_slug)
        .is_some_and(|template| connection_workflow_trigger_supported(roots, connection, template))
}

fn write_runs(out: &mut String, runs: &[WorkflowRun], query: &str) {
    out.push_str("\nRuns\n");
    let matching = runs
        .iter()
        .filter(|run| {
            matches_query(
                query,
                [
                    run.workflow_slug.as_str(),
                    run.run_id.as_str(),
                    run.error.as_deref().unwrap_or_default(),
                ],
            )
        })
        .collect::<Vec<_>>();
    if matching.is_empty() && runs.is_empty() {
        out.push_str("- none recorded\n");
        return;
    }
    if matching.is_empty() {
        out.push_str("- no matching runs\n");
        return;
    }
    for run in matching.into_iter().take(20) {
        let _ = writeln!(
            out,
            "- #{} {} {:?} nodes={}{}",
            run.idx,
            run.workflow_slug,
            run.status,
            run.nodes.len(),
            run.error
                .as_deref()
                .map(|error| format!(" error={}", first_line(error)))
                .unwrap_or_default()
        );
    }
}

fn trigger_label(trigger: &TriggerSpec) -> String {
    match trigger {
        TriggerSpec::Cron { cron } => format!("cron:{cron}"),
        TriggerSpec::Subscription { source_topic, .. } => format!("connection:{source_topic}"),
        TriggerSpec::Connection {
            connection_slug, ..
        } => format!("connection:{connection_slug}"),
    }
}

fn first_line(text: &str) -> &str {
    text.lines().next().unwrap_or(text)
}

fn matches_query<'a>(query: &str, values: impl IntoIterator<Item = &'a str>) -> bool {
    let terms = search_terms(query);
    if terms.is_empty() {
        return true;
    }
    let haystack = values
        .into_iter()
        .map(str::to_ascii_lowercase)
        .collect::<Vec<_>>()
        .join(" ");
    terms.iter().all(|term| haystack.contains(term))
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn subscriber_manifest_roots(paths: &ConfigPaths) -> SubscriberManifestRoots {
    SubscriberManifestRoots::new(
        paths.workspace_config_dir.clone(),
        paths.user_config_dir.clone(),
        paths.builtin_resources_dir.clone(),
    )
}
