use crate::{subscription_manager, AppState};
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_subscriptions::{
    builtin_connector_templates, connection_workflow_trigger_supported, connector_runtime_hints,
    connector_workflow_trigger_supported, suggested_connection_slug, ActionSpec, ConnectionRecord,
    ConnectorTemplate, FilterSpec, SubscriberManifestRoots, TaggedFilterSpec, WorkflowBindingSpec,
};
use puffer_workflow::{TriggerSpec, WorkflowDefinition, WorkflowRun, WorkflowStore};
use std::cmp::Ordering;
use std::fmt::Write as _;

mod append;
mod create;
mod delete;
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
    let (mode, query) = split_workflows_args(args);

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
            | "actions"
            | "action"
            | "bindings"
            | "binding"
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
            write_workflows(&mut out, &workflows, &runs, &query);
            write_workflow_bindings(&mut out, &connector_context, "");
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
        "actions" | "action" | "bindings" | "binding" => {
            write_workflow_bindings(&mut out, &connector_context, &query);
        }
        "runs" | "run" => {
            write_runs(&mut out, &runs, &query);
        }
        "new" | "create" => {
            out.push_str(&create::create_workflow(&paths, &query)?);
        }
        "append" | "file-append" => {
            out.push_str(&append::create_append_binding(&paths, &query)?);
        }
        "delete" | "remove" | "rm" => {
            out.push_str(&delete::delete_workflow_binding(&query)?);
        }
        _ => {
            out.push_str(
                "Usage: /workflows [list|new|append|delete|actions|connections|connectors|tasks|runs] [query]\n\
                 Tip: /workflows append <connection-slug> <file-path> [pattern] creates an enabled file append binding; /workflows delete <binding-slug> removes one.",
            );
        }
    }
    Ok(out)
}

fn split_workflows_args(args: &str) -> (&str, &str) {
    let trimmed = args.trim();
    match trimmed.find(char::is_whitespace) {
        Some(index) => {
            let (mode, query) = trimmed.split_at(index);
            (mode, query.trim_start())
        }
        None => (trimmed, ""),
    }
}

struct ConnectorContext {
    connectors: Vec<ConnectorTemplate>,
    connections: Vec<ConnectionRecord>,
    bindings: Vec<WorkflowBindingSpec>,
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
                bindings: manager.store().list(),
                error,
            }
        }
        Err(error) => ConnectorContext {
            connectors: builtin_connector_templates(),
            connections: Vec::new(),
            bindings: Vec::new(),
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
        "workflows={} bindings={} runs={} connections={} connectors={} monitor_tasks={}/{}",
        workflows.len(),
        context.bindings.len(),
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

fn write_workflows(
    out: &mut String,
    workflows: &[WorkflowDefinition],
    runs: &[WorkflowRun],
    query: &str,
) {
    out.push_str("\nWorkflows\n");
    let matching = workflows
        .iter()
        .filter(|workflow| {
            matches_query(
                query,
                workflow_search_terms(workflow).iter().map(String::as_str),
            )
        })
        .collect::<Vec<_>>();
    write_result_summary(out, matching.len(), workflows.len(), "workflows", query);
    if matching.is_empty() && workflows.is_empty() {
        out.push_str("- none registered\n");
        return;
    }
    if matching.is_empty() {
        out.push_str("- no matching workflows\n");
        return;
    }
    for workflow in matching {
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

fn write_workflow_bindings(out: &mut String, context: &ConnectorContext, query: &str) {
    out.push_str("\nWorkflow actions\n");
    write_binding_filter_hints(out);
    let bindings = context
        .bindings
        .iter()
        .filter(|binding| {
            matches_query(
                query,
                binding_search_terms(binding).iter().map(String::as_str),
            )
        })
        .collect::<Vec<_>>();
    write_result_summary(
        out,
        bindings.len(),
        context.bindings.len(),
        "workflow actions",
        query,
    );
    if bindings.is_empty() && context.bindings.is_empty() {
        out.push_str(
            "- none configured; run /workflows append <connection-slug> <file-path> [pattern]\n",
        );
        return;
    }
    if bindings.is_empty() {
        out.push_str("- no matching workflow actions\n");
        return;
    }
    for binding in bindings {
        let _ = writeln!(
            out,
            "- {} [{}] connection={} connector={} action={}{} filter={} {}",
            binding.slug,
            workflow_status_label(binding.status),
            binding.connection_slug,
            binding.connector_slug.as_deref().unwrap_or("-"),
            workflow_action_type(&binding.action),
            workflow_action_target(&binding.action),
            workflow_filter_label(binding.filter.as_ref()),
            binding.description
        );
        let _ = writeln!(out, "  delete={}", workflow_delete_command(&binding.slug));
    }
}

fn write_binding_filter_hints(out: &mut String) {
    out.push_str(
        "filters: append | file | connection | connector | pattern | enabled | paused | delete\n",
    );
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
        let append = if trigger_supported {
            format!(
                " append={}",
                workflow_append_command(&connection.slug, None)
            )
        } else {
            String::new()
        };
        let connect_command = connection_connect_command(connection);
        let _ = writeln!(
            out,
            "- {} connector={} state={:?} {} {} repair={}{}{}{} {}",
            connection.slug,
            connection.connector_slug,
            connection.state,
            consumer,
            trigger,
            connect_command,
            draft,
            append,
            monitor,
            connection.description
        );
    }
}

fn write_connection_filter_hints(out: &mut String) {
    out.push_str(
        "filters: trigger-ready | no-trigger | draft | append | monitor | repair | active | idle\n",
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
            let append_command =
                trigger_supported.then(|| connector_append_command(context, connector));
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
                        append_command.as_deref().unwrap_or_default(),
                        "connect",
                        "setup",
                        "draft workflow new",
                        "append file save workflow",
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
    let show_unique_connector_detail = connectors.len() == 1 && !query.trim().is_empty();
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
        let append_summary = trigger_supported
            .then(|| format!(" append={}", connector_append_command(context, connector)))
            .unwrap_or_default();
        let _ = writeln!(
            out,
            "- {} [{}] {} runtime={}{}{} connect={}{}{}",
            connector.slug,
            capabilities.join(","),
            connector.description,
            runtime_summary,
            action_summary,
            connection_summary,
            connect_command,
            draft_summary,
            append_summary
        );
        write_connector_detail(out, connector, show_unique_connector_detail);
    }
}

fn write_connector_filter_hints(out: &mut String, include_all: bool) {
    if include_all {
        out.push_str(
            "filters: trigger-ready | no-trigger | draft | append | has-actions | serve | subscriber | internal-tool | command\n",
        );
    } else {
        out.push_str(
            "filters: /workflows connectors trigger-ready | no-trigger | draft | append | has-actions | serve | subscriber | internal-tool\n",
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

fn write_connector_detail(out: &mut String, connector: &ConnectorTemplate, show: bool) {
    if !show {
        return;
    }
    let action_slugs = connector_action_slugs(connector);
    if action_slugs.is_empty() {
        return;
    }
    let _ = writeln!(out, "  actions_all={}", action_slugs.join(","));
}

fn connection_connect_command(connection: &ConnectionRecord) -> String {
    format!("/connect {} {}", connection.connector_slug, connection.slug)
}

fn workflow_draft_command(connection_slug: &str) -> String {
    format!("/workflows new {connection_slug}-workflow {connection_slug}")
}

fn workflow_append_command(connection_slug: &str, connector_slug: Option<&str>) -> String {
    let base = format!("/workflows append {connection_slug} /tmp/{connection_slug}.log");
    match connector_slug {
        Some(connector_slug) => format!("{base} --connector {connector_slug}"),
        None => base,
    }
}

fn workflow_delete_command(binding_slug: &str) -> String {
    format!("/workflows delete {binding_slug}")
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

fn connector_append_command(context: &ConnectorContext, connector: &ConnectorTemplate) -> String {
    if let Some(connection) = context
        .connections
        .iter()
        .find(|connection| connection.connector_slug == connector.slug)
    {
        workflow_append_command(&connection.slug, None)
    } else {
        workflow_append_command(
            &suggested_connection_slug(&connector.slug),
            Some(&connector.slug),
        )
    }
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
                "append",
                "file",
                "save",
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
        terms.push(workflow_append_command(&connection.slug, None));
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

fn binding_search_terms(binding: &WorkflowBindingSpec) -> Vec<String> {
    [
        "workflow action binding".to_string(),
        binding.slug.clone(),
        binding.description.clone(),
        binding.connection_slug.clone(),
        binding.connector_slug.clone().unwrap_or_default(),
        workflow_status_label(binding.status).to_string(),
        workflow_action_type(&binding.action).to_string(),
        workflow_action_target(&binding.action),
        workflow_filter_label(binding.filter.as_ref()),
        workflow_delete_command(&binding.slug),
        "delete remove cleanup".to_string(),
    ]
    .into_iter()
    .filter(|term| !term.is_empty())
    .collect()
}

fn workflow_search_terms(workflow: &WorkflowDefinition) -> Vec<String> {
    let mut terms = vec![
        workflow.slug.clone(),
        workflow.pipeline.name.clone(),
        trigger_label(&workflow.trigger),
        if workflow.enabled {
            "enabled".to_string()
        } else {
            "paused".to_string()
        },
    ];
    for node in &workflow.pipeline.nodes {
        terms.push(node.id.clone());
        terms.push(node.node_type.clone().unwrap_or_default());
        terms.push(node.agent.clone().unwrap_or_default());
        terms.push(node.prompt.clone());
        terms.push(node.model.clone().unwrap_or_default());
        terms.extend(node.tools.iter().cloned());
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

fn workflow_status_label(status: puffer_subscriptions::WorkflowBindingStatus) -> &'static str {
    match status {
        puffer_subscriptions::WorkflowBindingStatus::Enabled => "enabled",
        puffer_subscriptions::WorkflowBindingStatus::Paused => "paused",
    }
}

fn workflow_action_type(action: &ActionSpec) -> &'static str {
    match action {
        ActionSpec::SqliteInsert { .. } => "sqlite_insert",
        ActionSpec::FileAppend { .. } => "file_append",
        ActionSpec::ForwardMessage { .. } => "forward_message",
        ActionSpec::RunWorkflow { .. } => "run_workflow",
        ActionSpec::ConnectorAct { .. } => "connector_act",
        ActionSpec::ToolCall { .. } => "tool_call",
        ActionSpec::TriageAgent { .. } => "triage_agent",
        ActionSpec::Graph { .. } => "graph",
        ActionSpec::Unknown => "unknown",
    }
}

fn workflow_action_target(action: &ActionSpec) -> String {
    match action {
        ActionSpec::SqliteInsert { path, table } => format!(" path={path} table={table}"),
        ActionSpec::FileAppend { path, .. } => format!(" path={path}"),
        ActionSpec::ForwardMessage {
            platform, target, ..
        } => {
            format!(" platform={platform} target={target}")
        }
        ActionSpec::RunWorkflow { slug } => format!(" workflow={slug}"),
        ActionSpec::ConnectorAct {
            connector_slug,
            action,
            ..
        } => format!(" connector={connector_slug} action={action}"),
        ActionSpec::ToolCall { tool, .. } => format!(" tool={tool}"),
        ActionSpec::TriageAgent { .. } | ActionSpec::Graph { .. } | ActionSpec::Unknown => {
            String::new()
        }
    }
}

fn workflow_filter_label(filter: Option<&FilterSpec>) -> String {
    match filter {
        Some(FilterSpec::Tagged(TaggedFilterSpec::Regex { pattern, .. })) => pattern.clone(),
        Some(FilterSpec::Tagged(TaggedFilterSpec::Jq { expression })) => expression.clone(),
        Some(FilterSpec::Json(shape)) => shape.to_string(),
        None => ".*".to_string(),
    }
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
        .filter(|run| matches_query(query, run_search_terms(run).iter().map(String::as_str)))
        .collect::<Vec<_>>();
    write_result_summary(out, matching.len(), runs.len(), "runs", query);
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

fn run_search_terms(run: &WorkflowRun) -> Vec<String> {
    let mut terms = vec![
        run.workflow_slug.clone(),
        run.run_id.clone(),
        format!("{:?}", run.status).to_ascii_lowercase(),
        run.error.clone().unwrap_or_default(),
        run.trigger.to_string(),
        run.trigger_key.clone().unwrap_or_default(),
    ];
    for node in &run.nodes {
        terms.push(node.id.clone());
        terms.push(format!("{:?}", node.status).to_ascii_lowercase());
        terms.push(node.output.clone().unwrap_or_default());
        terms.push(node.error.clone().unwrap_or_default());
    }
    terms.into_iter().filter(|term| !term.is_empty()).collect()
}

fn trigger_label(trigger: &TriggerSpec) -> String {
    match trigger {
        TriggerSpec::Cron { cron } => format!("cron:{cron}"),
        TriggerSpec::Subscription { source_topic, .. } => {
            format!("subscription:{source_topic}")
        }
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
