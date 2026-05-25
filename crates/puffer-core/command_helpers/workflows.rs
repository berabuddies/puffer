use crate::{subscription_manager, AppState};
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    builtin_connector_templates, connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, suggested_connection_slug, ConnectionRecord,
    ConnectorTemplate, SubscriberManifestRoots,
};
use puffer_workflow::{TriggerSpec, WorkflowDefinition, WorkflowRun, WorkflowStore};
use std::cmp::Ordering;
use std::fmt::Write as _;

mod create;
mod monitor_tasks;

/// Renders a terminal-friendly workflow, connection, and connector summary.
pub(crate) fn handle_workflows_command(state: &AppState, args: &str) -> Result<String> {
    let paths = ConfigPaths::discover(&state.cwd);
    let store = WorkflowStore::new(&paths.workspace_config_dir);
    let workflows = store.list()?;
    let runs = store.list_runs()?;
    let connector_context = connector_context();
    let monitor_task_context = monitor_tasks::load_monitor_task_context(&paths);
    let roots = subscriber_manifest_roots(&paths);
    let mut parts = args.split_whitespace();
    let mode = parts.next().unwrap_or_default();
    let query = parts.collect::<Vec<_>>().join(" ");

    let mut out = String::new();
    if matches!(
        mode,
        "" | "show"
            | "list"
            | "connections"
            | "connection"
            | "connectors"
            | "connector"
            | "tasks"
            | "task"
            | "monitors"
            | "monitor-tasks"
            | "runs"
            | "run"
    ) {
        write_header(
            &mut out,
            &paths,
            &workflows,
            &runs,
            &connector_context,
            &monitor_task_context,
        );
    }
    match mode {
        "" | "show" | "list" => {
            write_workflows(&mut out, &workflows, &runs);
            write_connections(&mut out, &connector_context, &roots, "");
            monitor_tasks::write_monitor_tasks(&mut out, &monitor_task_context, "");
            write_connectors(&mut out, &connector_context, &roots, false, "");
        }
        "connections" | "connection" => {
            write_connections(&mut out, &connector_context, &roots, &query);
        }
        "connectors" | "connector" => {
            write_connectors(&mut out, &connector_context, &roots, true, &query);
        }
        "tasks" | "task" | "monitors" | "monitor-tasks" => {
            monitor_tasks::write_monitor_tasks(&mut out, &monitor_task_context, &query);
        }
        "runs" | "run" => {
            write_runs(&mut out, &runs, &query);
        }
        "new" | "create" => {
            out.push_str(&create::create_workflow(&paths, &query)?);
        }
        _ => {
            out.push_str(
                "Usage: /workflows [list|new|connections|connectors|tasks|runs] [query]\n\
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
    monitor_tasks: &monitor_tasks::MonitorTaskContext,
) {
    let _ = writeln!(out, "Workflow dashboard");
    let _ = writeln!(out, "workspace={}", paths.workspace_root.display());
    let _ = writeln!(
        out,
        "workflows={} runs={} connections={} connectors={} monitor_tasks={}/{}",
        workflows.len(),
        runs.len(),
        context.connections.len(),
        context.connectors.len(),
        monitor_tasks.active_count(),
        monitor_tasks.tasks.len()
    );
    if let Some(error) = &context.error {
        let _ = writeln!(out, "connector_runtime={}", first_line(error));
    }
    if let Some(error) = &monitor_tasks.error {
        let _ = writeln!(out, "monitor_tasks={}", first_line(error));
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
            let terms = connection_search_terms(context, roots, connection);
            matches_query(query, terms.iter().map(String::as_str))
        })
        .collect::<Vec<_>>();
    write_connection_filter_hints(out);
    write_result_summary(
        out,
        connections.len(),
        context.connections.len(),
        "connections",
        query,
    );
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
        let trigger_supported = connection_trigger_supported(context, roots, connection);
        let trigger = if trigger_supported {
            "trigger=ready"
        } else {
            "trigger=unavailable"
        };
        let monitor = if trigger_supported {
            format!(" monitor=/monitor {}", connection.slug)
        } else {
            String::new()
        };
        let draft = if trigger_supported {
            format!(" draft={}", workflow_draft_command(&connection.slug))
        } else {
            String::new()
        };
        let connect_command = connection_connect_command(connection);
        let _ = writeln!(
            out,
            "- {} connector={} state={:?} {} {} repair={}{}{} {}",
            connection.slug,
            connection.connector_slug,
            connection.state,
            consumer,
            trigger,
            connect_command,
            draft,
            monitor,
            connection.description
        );
    }
}

fn write_connection_filter_hints(out: &mut String) {
    out.push_str(
        "filters: trigger-ready | no-trigger | draft | monitor | repair | active | idle\n",
    );
}

fn write_connectors(
    out: &mut String,
    context: &ConnectorContext,
    roots: &SubscriberManifestRoots,
    include_all: bool,
    query: &str,
) {
    out.push_str("\nConnectors\n");
    write_connector_filter_hints(out, include_all);
    let candidates = context
        .connectors
        .iter()
        .filter(|connector| include_all || connector_workflow_trigger_supported(roots, connector))
        .collect::<Vec<_>>();
    let connectors = candidates
        .iter()
        .copied()
        .filter(|connector| {
            let action_slugs = connector_action_slugs(connector);
            let suggested_connection = suggested_connection_slug(&connector.slug);
            let connect_command = connector_connect_command(connector);
            let trigger_supported = connector_workflow_trigger_supported(roots, connector);
            let draft_command =
                trigger_supported.then(|| connector_draft_command(context, connector));
            let runtime_hints = connector_runtime_hints(roots, connector);
            let action_capability = if connector.actions.is_empty() {
                ""
            } else {
                "has-actions"
            };
            let connection_values = context
                .connections
                .iter()
                .filter(|connection| connection.connector_slug == connector.slug)
                .flat_map(|connection| [connection.slug.as_str(), connection.description.as_str()]);
            let trigger_terms = if trigger_supported {
                ["trigger", "trigger-ready", ""]
            } else {
                ["no-trigger", "no trigger", "setup-only"]
            };
            matches_query(
                query,
                action_slugs
                    .into_iter()
                    .chain([
                        connector.slug.as_str(),
                        connector.description.as_str(),
                        connector.skill.as_str(),
                        suggested_connection.as_str(),
                        connect_command.as_str(),
                        draft_command.as_deref().unwrap_or_default(),
                        "connect",
                        "setup",
                        "draft workflow new",
                        action_capability,
                    ])
                    .filter(|term| !term.is_empty())
                    .chain(runtime_hints.iter().map(String::as_str))
                    .chain(trigger_terms.into_iter().filter(|term| !term.is_empty()))
                    .chain(connection_values),
            )
        })
        .collect::<Vec<_>>();
    write_result_summary(
        out,
        connectors.len(),
        candidates.len(),
        if include_all {
            "connectors"
        } else {
            "trigger-ready connectors"
        },
        query,
    );
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
        let trigger_supported = connector_workflow_trigger_supported(roots, connector);
        if trigger_supported {
            capabilities.push("trigger");
        } else if include_all {
            capabilities.push("no-trigger");
        }
        if !connector.actions.is_empty() {
            capabilities.push("actions");
        }
        let runtime_summary = connector_runtime_hints(roots, connector).join(",");
        let action_summary = connector_action_summary(connector, query)
            .map(|summary| format!(" actions={summary}"))
            .unwrap_or_default();
        let connection_summary = connector_connection_summary(context, connector)
            .map(|summary| format!(" connections={summary}"))
            .unwrap_or_default();
        let connect_command = connector_connect_command(connector);
        let draft_summary = trigger_supported
            .then(|| format!(" draft={}", connector_draft_command(context, connector)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} [{}] {} runtime={}{}{} connect={}{}",
            connector.slug,
            capabilities.join(","),
            connector.description,
            runtime_summary,
            action_summary,
            connection_summary,
            connect_command,
            draft_summary
        );
    }
}

fn write_connector_filter_hints(out: &mut String, include_all: bool) {
    if include_all {
        out.push_str(
            "filters: trigger-ready | no-trigger | draft | has-actions | serve | subscriber | internal-tool | command\n",
        );
    } else {
        out.push_str(
            "filters: /workflows connectors trigger-ready | no-trigger | draft | has-actions | serve | subscriber | internal-tool\n",
        );
    }
}

fn write_result_summary(out: &mut String, shown: usize, total: usize, label: &str, query: &str) {
    let query = query.trim();
    if query.is_empty() {
        let _ = writeln!(out, "showing {shown}/{total} {label}");
    } else {
        let _ = writeln!(out, "showing {shown}/{total} {label} for query={query:?}");
    }
}

fn connector_action_slugs(connector: &ConnectorTemplate) -> Vec<&str> {
    let mut slugs = connector
        .actions
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    slugs.sort_by(
        |left, right| match (*left == "send_message", *right == "send_message") {
            (true, false) => Ordering::Less,
            (false, true) => Ordering::Greater,
            _ => left.cmp(right),
        },
    );
    slugs
}

fn connection_connect_command(connection: &ConnectionRecord) -> String {
    format!("/connect {} {}", connection.connector_slug, connection.slug)
}

fn workflow_draft_command(connection_slug: &str) -> String {
    format!("/workflows new {connection_slug}-workflow {connection_slug}")
}

fn connector_draft_command(context: &ConnectorContext, connector: &ConnectorTemplate) -> String {
    let connection_slug = context
        .connections
        .iter()
        .find(|connection| connection.connector_slug == connector.slug)
        .map(|connection| connection.slug.clone())
        .unwrap_or_else(|| suggested_connection_slug(&connector.slug));
    workflow_draft_command(&connection_slug)
}

fn connection_search_terms(
    context: &ConnectorContext,
    roots: &SubscriberManifestRoots,
    connection: &ConnectionRecord,
) -> Vec<String> {
    let connect_command = connection_connect_command(connection);
    let trigger_supported = connection_trigger_supported(context, roots, connection);
    let mut terms = vec![
        connection.slug.clone(),
        connection.connector_slug.clone(),
        connection.description.clone(),
        format!("{:?}", connection.state).to_ascii_lowercase(),
        connect_command,
        "connect".to_string(),
        "repair".to_string(),
        "reconnect".to_string(),
    ];
    if connection.has_consumer {
        terms.extend(["consumer", "active"].into_iter().map(str::to_string));
    } else {
        terms.extend(["consumer", "idle"].into_iter().map(str::to_string));
    }
    if trigger_supported {
        terms.extend(
            [
                "trigger",
                "trigger-ready",
                "draft",
                "new",
                "workflow",
                "monitor",
                "monitorable",
            ]
            .into_iter()
            .map(str::to_string),
        );
        terms.push(format!("/monitor {}", connection.slug));
        terms.push(workflow_draft_command(&connection.slug));
    } else {
        terms.extend(
            ["no trigger", "no-trigger", "setup-only"]
                .into_iter()
                .map(str::to_string),
        );
    }
    if let Some(connector) = context
        .connectors
        .iter()
        .find(|template| template.slug == connection.connector_slug)
    {
        terms.extend(
            [
                connector.description.clone(),
                connector.skill.clone(),
                connector_runtime_hints(roots, connector).join(" "),
                connector_action_slugs(connector).join(" "),
            ]
            .into_iter()
            .filter(|term| !term.is_empty()),
        );
    }
    terms
}

fn connector_connect_command(connector: &ConnectorTemplate) -> String {
    format!(
        "/connect {} {}",
        connector.slug,
        suggested_connection_slug(&connector.slug)
    )
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

fn connector_connection_summary(
    context: &ConnectorContext,
    connector: &ConnectorTemplate,
) -> Option<String> {
    let connections = context
        .connections
        .iter()
        .filter(|connection| connection.connector_slug == connector.slug)
        .collect::<Vec<_>>();
    if connections.is_empty() {
        return None;
    }
    let visible = connections
        .iter()
        .take(3)
        .map(|connection| connection.slug.as_str())
        .collect::<Vec<_>>();
    let hidden = connections.len().saturating_sub(visible.len());
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

#[cfg(test)]
include!("workflows/tests.rs");
