use super::{ConnectorPopupRow, PopupRow, MAX_POPUP_ROWS};
use puffer_core::subscription_manager;
use puffer_subscriptions::{
    ActionSpec, FilterSpec, TaggedFilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};

/// Returns workflow-action management completion rows for `/workflows ...`.
pub(super) fn workflow_action_rows(input: &str) -> Option<Vec<PopupRow>> {
    let rest = input
        .strip_prefix('/')?
        .split_once(' ')
        .filter(|(command, _)| is_workflows_command(command))?
        .1
        .trim_start();
    workflow_action_rows_for_bindings(rest, live_workflow_bindings())
}

/// Returns true when workflow-action management should keep the popup open.
pub(super) fn workflow_action_accepts_input(rest: &str) -> bool {
    workflow_action_query(rest).is_some()
}

fn workflow_action_rows_for_bindings(
    rest: &str,
    bindings: Vec<WorkflowBindingSpec>,
) -> Option<Vec<PopupRow>> {
    let (command, query) = workflow_action_query(rest)?;
    let terms = search_terms(query);
    let mut rows = bindings
        .into_iter()
        .filter(|binding| command.applies_to(binding))
        .map(|binding| workflow_binding_action_row(command, binding))
        .filter(|row| terms.iter().all(|term| row.search_text.contains(term)))
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| workflow_action_sort_key(row, command, query.trim()));
    rows.truncate(MAX_POPUP_ROWS);
    Some(rows.into_iter().map(|row| row.row).collect())
}

fn workflow_action_query(rest: &str) -> Option<(WorkflowActionCommand, &str)> {
    let (subcommand, query) = split_first_whitespace(rest)?;
    let command = WorkflowActionCommand::from_subcommand(subcommand)?;
    Some((command, query.trim_start()))
}

fn split_first_whitespace(value: &str) -> Option<(&str, &str)> {
    let index = value.find(char::is_whitespace)?;
    Some((&value[..index], &value[index..]))
}

fn live_workflow_bindings() -> Vec<WorkflowBindingSpec> {
    let Ok(manager) = subscription_manager() else {
        return Vec::new();
    };
    manager.store().list()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WorkflowActionCommand {
    Pause,
    Resume,
    Delete,
}

impl WorkflowActionCommand {
    fn from_subcommand(subcommand: &str) -> Option<Self> {
        match subcommand {
            "pause" | "disable" => Some(Self::Pause),
            "resume" | "enable" => Some(Self::Resume),
            "delete" | "remove" | "rm" => Some(Self::Delete),
            _ => None,
        }
    }

    fn canonical(self) -> &'static str {
        match self {
            Self::Pause => "pause",
            Self::Resume => "resume",
            Self::Delete => "delete",
        }
    }

    fn verb(self) -> &'static str {
        match self {
            Self::Pause => "Pause",
            Self::Resume => "Resume",
            Self::Delete => "Delete",
        }
    }

    fn search_terms(self) -> &'static str {
        match self {
            Self::Pause => "pause disable stop suspend",
            Self::Resume => "resume enable start unpause",
            Self::Delete => "delete remove rm cleanup",
        }
    }

    fn applies_to(self, binding: &WorkflowBindingSpec) -> bool {
        match self {
            Self::Pause => binding.status == WorkflowBindingStatus::Enabled,
            Self::Resume => binding.status == WorkflowBindingStatus::Paused,
            Self::Delete => true,
        }
    }
}

fn workflow_binding_action_row(
    action_command: WorkflowActionCommand,
    binding: WorkflowBindingSpec,
) -> ConnectorPopupRow {
    let command = format!("/workflows {} {}", action_command.canonical(), binding.slug);
    let description = workflow_binding_action_description(action_command, &binding);
    let search_text =
        workflow_binding_action_search_text(action_command, &binding, &command, &description);
    ConnectorPopupRow {
        row: PopupRow {
            name: format!("workflows {} {}", action_command.canonical(), binding.slug),
            description,
            replacement: command,
            append_space: false,
        },
        search_text,
    }
}

fn workflow_binding_action_description(
    action_command: WorkflowActionCommand,
    binding: &WorkflowBindingSpec,
) -> String {
    format!(
        "{} {} action  connection={}; connector={}; status={}; target={}; filter={}",
        action_command.verb(),
        workflow_action_type(&binding.action),
        binding.connection_slug,
        binding.connector_slug.as_deref().unwrap_or("-"),
        workflow_status_label(binding.status),
        workflow_action_target(&binding.action),
        workflow_filter_label(binding.filter.as_ref())
    )
}

fn workflow_binding_action_search_text(
    action_command: WorkflowActionCommand,
    binding: &WorkflowBindingSpec,
    command: &str,
    description: &str,
) -> String {
    [
        format!("workflow action binding {}", action_command.search_terms()),
        binding.slug.clone(),
        binding.description.clone(),
        binding.connection_slug.clone(),
        binding.connector_slug.clone().unwrap_or_default(),
        workflow_status_label(binding.status).to_string(),
        workflow_action_type(&binding.action).to_string(),
        workflow_action_target(&binding.action),
        workflow_filter_label(binding.filter.as_ref()),
        command.to_string(),
        description.to_string(),
    ]
    .into_iter()
    .filter(|term| !term.is_empty())
    .collect::<Vec<_>>()
    .join(" ")
    .to_ascii_lowercase()
}

fn workflow_action_sort_key(
    row: &ConnectorPopupRow,
    command: WorkflowActionCommand,
    query: &str,
) -> (u8, String) {
    let name = row.row.name.to_ascii_lowercase();
    let description = row.row.description.to_ascii_lowercase();
    let query = query.to_ascii_lowercase();
    let canonical = command.canonical();
    if query.is_empty() {
        return (0, name);
    }
    if name == format!("workflows {canonical} {query}") || name == query {
        return (0, name);
    }
    if name.starts_with(&format!("workflows {canonical} {query}")) || name.starts_with(&query) {
        return (1, name);
    }
    if name.contains(&query) {
        return (2, name);
    }
    if description.contains(&query) {
        return (3, name);
    }
    (4, name)
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn is_workflows_command(command: &str) -> bool {
    matches!(command, "workflow" | "workflows" | "pipeline" | "pipelines")
}

fn workflow_status_label(status: WorkflowBindingStatus) -> &'static str {
    match status {
        WorkflowBindingStatus::Enabled => "enabled",
        WorkflowBindingStatus::Paused => "paused",
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
        ActionSpec::SqliteInsert { path, table } => format!("path={path} table={table}"),
        ActionSpec::FileAppend { path, .. } => format!("path={path}"),
        ActionSpec::ForwardMessage {
            platform, target, ..
        } => {
            format!("platform={platform} target={target}")
        }
        ActionSpec::RunWorkflow { slug } => format!("workflow={slug}"),
        ActionSpec::ConnectorAct {
            connector_slug,
            action,
            ..
        } => format!("connector={connector_slug} action={action}"),
        ActionSpec::ToolCall { tool, .. } => format!("tool={tool}"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn delete_rows_match_binding_metadata() {
        let rows = workflow_action_rows_for_bindings("delete email hi", vec![sample_binding()])
            .expect("rows");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "workflows delete append-email-hi");
        assert_eq!(rows[0].replacement, "/workflows delete append-email-hi");
        assert!(rows[0].description.contains("connection=email"));
        assert!(rows[0].description.contains("target=path=messages.db"));
    }

    #[test]
    fn delete_rows_ignore_other_workflow_subcommands() {
        assert!(
            workflow_action_rows_for_bindings("actions email", vec![sample_binding()]).is_none()
        );
    }

    #[test]
    fn delete_rows_accept_remove_aliases() {
        let rows =
            workflow_action_rows_for_bindings("rm paused", vec![paused_binding()]).expect("rows");

        assert_eq!(rows[0].replacement, "/workflows delete append-paused");
    }

    #[test]
    fn pause_rows_only_offer_enabled_bindings() {
        let rows = workflow_action_rows_for_bindings(
            "disable email",
            vec![sample_binding(), paused_binding()],
        )
        .expect("rows");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "workflows pause append-email-hi");
        assert_eq!(rows[0].replacement, "/workflows pause append-email-hi");
        assert!(rows[0].description.contains("Pause sqlite_insert action"));
    }

    #[test]
    fn resume_rows_only_offer_paused_bindings() {
        let rows = workflow_action_rows_for_bindings(
            "enable paused",
            vec![sample_binding(), paused_binding()],
        )
        .expect("rows");

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "workflows resume append-paused");
        assert_eq!(rows[0].replacement, "/workflows resume append-paused");
        assert!(rows[0].description.contains("Resume sqlite_insert action"));
    }

    fn sample_binding() -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: "append-email-hi".to_string(),
            description: "Append email messages matching hi".to_string(),
            connection_slug: "email".to_string(),
            connector_slug: Some("email".to_string()),
            status: WorkflowBindingStatus::Enabled,
            filter: Some(FilterSpec::Tagged(TaggedFilterSpec::Regex {
                pattern: "hi".to_string(),
                case_insensitive: true,
            })),
            classify_prompt: None,
            classify_model: None,
            action: ActionSpec::SqliteInsert {
                path: "messages.db".to_string(),
                table: "events".to_string(),
            },
            created_at_ms: 0,
        }
    }

    fn paused_binding() -> WorkflowBindingSpec {
        WorkflowBindingSpec {
            slug: "append-paused".to_string(),
            status: WorkflowBindingStatus::Paused,
            ..sample_binding()
        }
    }
}
