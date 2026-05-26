#[path = "popup_connections.rs"]
mod popup_connections;

use self::popup_connections::{
    live_connect_connection_rows, live_monitor_connection_rows, live_workflow_connection_rows,
    WorkflowConnectorQuery,
};
use puffer_core::CommandSpec;
use puffer_subscriptions::{
    builtin_connector_templates, suggested_connection_slug, ConnectorTemplate,
};
use std::collections::BTreeSet;

const MAX_POPUP_ROWS: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct PopupRow {
    pub(crate) name: String,
    pub(crate) description: String,
    pub(crate) replacement: String,
    pub(crate) append_space: bool,
}

struct WorkflowSubcommand {
    name: &'static str,
    description: &'static str,
    hint: Option<&'static str>,
    search_terms: &'static [&'static str],
}

struct ConnectorPopupRow {
    row: PopupRow,
    search_text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkflowConnectorCommandKind {
    New,
    Append,
}

const WORKFLOW_SUBCOMMANDS: &[WorkflowSubcommand] = &[
    WorkflowSubcommand {
        name: "list",
        description: "Show workflow definitions and latest run status",
        hint: Some("[query]"),
        search_terms: &["show", "workflow", "pipeline", "definition", "status"],
    },
    WorkflowSubcommand {
        name: "new",
        description: "Create a workflow draft for a trigger-ready connection",
        hint: Some("[slug] [connection-slug] [pattern]"),
        search_terms: &["create", "draft", "trigger", "pattern", "pipeline"],
    },
    WorkflowSubcommand {
        name: "append",
        description: "Create a file append action for matching connector events",
        hint: Some("<connection-slug> <file-path> [pattern]"),
        search_terms: &["file", "save", "write", "action", "binding"],
    },
    WorkflowSubcommand {
        name: "delete",
        description: "Remove a workflow action binding",
        hint: Some("<binding-slug>"),
        search_terms: &["remove", "rm", "cleanup", "binding", "action"],
    },
    WorkflowSubcommand {
        name: "actions",
        description: "Search workflow action bindings and delete commands",
        hint: Some("[query]"),
        search_terms: &["bindings", "append", "file", "delete", "pattern"],
    },
    WorkflowSubcommand {
        name: "connections",
        description: "Search connector connections and draft or append commands",
        hint: Some("[query]"),
        search_terms: &["connection", "trigger-ready", "repair", "monitor"],
    },
    WorkflowSubcommand {
        name: "connectors",
        description: "Search connector catalog by app, capability, action, or runtime",
        hint: Some("[query]"),
        search_terms: &["connector", "catalog", "apps", "capability", "runtime"],
    },
    WorkflowSubcommand {
        name: "tasks",
        description: "Search connector monitor tasks and task actions",
        hint: Some("[query]"),
        search_terms: &["task", "monitor", "ignored", "actions"],
    },
    WorkflowSubcommand {
        name: "runs",
        description: "Search workflow runs by id, status, trigger, output, or error",
        hint: Some("[query]"),
        search_terms: &["run", "history", "status", "trigger", "output", "error"],
    },
];

/// Returns slash-command popup rows for the current slash-input prefix.
pub(crate) fn popup_rows(input: &str, commands: &[CommandSpec]) -> Vec<PopupRow> {
    if let Some(rows) = workflow_connector_command_rows(input) {
        return rows;
    }
    if let Some(rows) = monitor_connector_rows(input) {
        return rows;
    }
    if let Some(rows) = workflow_subcommand_rows(input) {
        return rows;
    }
    if let Some(rows) = connector_catalog_rows(input) {
        return rows;
    }
    if !input.starts_with('/') || input.contains(' ') {
        return Vec::new();
    }
    let filter = input.trim_start_matches('/').to_ascii_lowercase();
    let mut rows = commands
        .iter()
        .filter(|command| !command.hidden)
        .filter(|command| command_matches(command, &filter))
        .collect::<Vec<_>>();
    rows.sort_by_key(|command| command_sort_key(command, &filter));
    rows.truncate(MAX_POPUP_ROWS);
    rows.into_iter().map(command_row).collect()
}

/// Returns true when the current input should render the slash popup.
pub(crate) fn popup_accepts_input(input: &str) -> bool {
    if !input.starts_with('/') {
        return false;
    }
    let Some((command, rest)) = input.trim_start_matches('/').split_once(' ') else {
        return true;
    };
    if is_workflows_command(command) {
        let rest = rest.trim_start();
        return workflow_connector_command_accepts(rest) || !rest.contains(char::is_whitespace);
    }
    command == "monitor" || command == "connect" && !connect_has_explicit_connection_name(rest)
}

/// Returns true when the input already exactly matches the selected popup row.
pub(crate) fn popup_row_matches_input(
    row: &PopupRow,
    input: &str,
    commands: &[CommandSpec],
) -> bool {
    let normalized = input.trim_end();
    if normalized == row.replacement {
        return true;
    }
    if !row.name.contains(' ') {
        let token = normalized.trim_start_matches('/');
        return commands.iter().any(|command| {
            command.name == row.name
                && (command.name == token || command.aliases.iter().any(|alias| alias == token))
        });
    }
    let parts = normalized
        .trim_start_matches('/')
        .split_whitespace()
        .collect::<Vec<_>>();
    parts.len() == 2
        && matches!(
            parts[0],
            "workflow" | "workflows" | "pipeline" | "pipelines"
        )
        && row.name == format!("workflows {}", parts[1])
}

fn command_row(command: &CommandSpec) -> PopupRow {
    PopupRow {
        name: command.name.clone(),
        description: command
            .argument_hint
            .as_deref()
            .map(|hint| format!("{}  {hint}", command.description))
            .unwrap_or_else(|| command.description.clone()),
        replacement: format!("/{}", command.name),
        append_space: command.argument_hint.is_some(),
    }
}

fn command_matches(command: &CommandSpec, filter: &str) -> bool {
    filter.is_empty()
        || command.name.starts_with(filter)
        || command
            .aliases
            .iter()
            .any(|alias| alias.starts_with(filter))
        || command.name.contains(filter)
        || command.aliases.iter().any(|alias| alias.contains(filter))
        || command.description.to_ascii_lowercase().contains(filter)
        || command
            .argument_hint
            .as_deref()
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(filter)
}

fn command_sort_key(command: &CommandSpec, filter: &str) -> (u8, String) {
    if filter.is_empty() {
        return (0, command.name.to_string());
    }
    if command.name == filter || command.aliases.iter().any(|alias| alias == filter) {
        return (0, command.name.to_string());
    }
    if command.name.starts_with(filter) {
        return (1, command.name.to_string());
    }
    if command
        .aliases
        .iter()
        .any(|alias| alias.starts_with(filter))
    {
        return (2, command.name.to_string());
    }
    if command.name.contains(filter) {
        return (3, command.name.to_string());
    }
    if command.aliases.iter().any(|alias| alias.contains(filter)) {
        return (4, command.name.to_string());
    }
    if command.description.to_ascii_lowercase().contains(filter) {
        return (5, command.name.to_string());
    }
    (6, command.name.to_string())
}

fn row_sort_key(row: &PopupRow, filter: &str) -> (u8, String) {
    if filter.is_empty() {
        return (0, row.name.to_string());
    }
    if row.name == filter {
        return (0, row.name.to_string());
    }
    if row.name.starts_with(filter) {
        return (1, row.name.to_string());
    }
    if row.name.contains(filter) {
        return (3, row.name.to_string());
    }
    if row.description.to_ascii_lowercase().contains(filter) {
        return (5, row.name.to_string());
    }
    (6, row.name.to_string())
}

fn workflow_subcommand_rows(input: &str) -> Option<Vec<PopupRow>> {
    let trimmed = input.strip_prefix('/')?;
    let (command, rest) = trimmed.split_once(' ')?;
    if !is_workflows_command(command) {
        return None;
    }
    if rest.trim_start().contains(char::is_whitespace) {
        return Some(Vec::new());
    }
    let filter = rest.trim_start().to_ascii_lowercase();
    let mut rows = WORKFLOW_SUBCOMMANDS
        .iter()
        .filter(|subcommand| workflow_subcommand_matches(subcommand, &filter))
        .map(workflow_subcommand_row)
        .collect::<Vec<_>>();
    rows.sort_by_key(|row| row_sort_key(row, &filter));
    rows.truncate(MAX_POPUP_ROWS);
    Some(rows)
}

fn workflow_connector_command_rows(input: &str) -> Option<Vec<PopupRow>> {
    let rest = input
        .strip_prefix('/')?
        .split_once(' ')
        .filter(|(command, _)| is_workflows_command(command))?
        .1
        .trim_start();
    let (kind, query) = workflow_connector_command_query(rest)?;
    let query_filter = WorkflowConnectorQuery::new(kind, query);
    let mut rows = live_workflow_connection_rows(kind)
        .into_iter()
        .filter_map(|row| query_filter.prepare(row))
        .collect::<Vec<_>>();
    let live_connections = rows
        .iter()
        .map(|row| row.row.name.clone())
        .collect::<BTreeSet<_>>();
    rows.extend(
        builtin_connector_templates()
            .into_iter()
            .filter(template_supports_workflow_command)
            .map(|template| workflow_connector_command_row(kind, template))
            .filter_map(|row| query_filter.prepare(row))
            .filter(|row| !live_connections.contains(&row.row.name)),
    );
    rows.sort_by_key(|row| connector_row_sort_key(row, query.trim()));
    rows.truncate(MAX_POPUP_ROWS);
    Some(rows.into_iter().map(|row| row.row).collect())
}

fn monitor_connector_rows(input: &str) -> Option<Vec<PopupRow>> {
    let rest = input
        .strip_prefix('/')?
        .split_once(' ')
        .filter(|(command, _)| *command == "monitor")?
        .1;
    let terms = search_terms(rest);
    let mut rows = live_monitor_connection_rows()
        .into_iter()
        .filter(|row| terms.iter().all(|term| row.search_text.contains(term)))
        .collect::<Vec<_>>();
    let live_connections = rows
        .iter()
        .map(|row| row.row.name.clone())
        .collect::<BTreeSet<_>>();
    rows.extend(
        builtin_connector_templates()
            .into_iter()
            .filter(template_supports_event_workflow)
            .map(monitor_connector_row)
            .filter(|row| terms.iter().all(|term| row.search_text.contains(term)))
            .filter(|row| !live_connections.contains(&row.row.name)),
    );
    rows.sort_by_key(|row| connector_command_row_sort_key(row, rest.trim(), "monitor"));
    rows.truncate(MAX_POPUP_ROWS);
    Some(rows.into_iter().map(|row| row.row).collect())
}

fn workflow_connector_command_accepts(rest: &str) -> bool {
    workflow_connector_command_query(rest).is_some()
}

fn workflow_connector_command_query(rest: &str) -> Option<(WorkflowConnectorCommandKind, &str)> {
    let (subcommand, query) = split_first_whitespace(rest)?;
    let kind = match subcommand {
        "new" => WorkflowConnectorCommandKind::New,
        "append" => WorkflowConnectorCommandKind::Append,
        _ => return None,
    };
    (!workflow_connector_command_complete(kind, query)).then_some((kind, query.trim_start()))
}

fn split_first_whitespace(value: &str) -> Option<(&str, &str)> {
    let index = value.find(char::is_whitespace)?;
    Some((&value[..index], &value[index..]))
}

fn workflow_connector_command_complete(kind: WorkflowConnectorCommandKind, query: &str) -> bool {
    let tokens = query.split_whitespace().collect::<Vec<_>>();
    match kind {
        WorkflowConnectorCommandKind::New => {
            tokens.len() >= 3
                || (tokens.len() >= 2
                    && tokens
                        .first()
                        .is_some_and(|slug| slug.ends_with("-workflow")))
        }
        WorkflowConnectorCommandKind::Append => {
            tokens.len() >= 2
                && tokens
                    .get(1)
                    .is_some_and(|path| looks_like_workflow_append_path(path))
        }
    }
}

fn looks_like_workflow_append_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.contains('/')
        || value.contains('.')
}

fn template_supports_event_workflow(template: &ConnectorTemplate) -> bool {
    (template.can_subscribe && (template.command_argv().is_some() || template.subscriber.is_some()))
        || template.slug == "email"
}

fn template_supports_workflow_command(template: &ConnectorTemplate) -> bool {
    template_supports_event_workflow(template)
}

fn workflow_connector_command_row(
    kind: WorkflowConnectorCommandKind,
    template: ConnectorTemplate,
) -> ConnectorPopupRow {
    let suggested_connection = suggested_connection_slug(&template.slug);
    let slug = template.slug.clone();
    let command = workflow_connector_command(kind, &template, &suggested_connection);
    let description =
        workflow_connector_command_description(kind, &template, &suggested_connection);
    let search_text = workflow_connector_command_search_text(&template, &suggested_connection);
    ConnectorPopupRow {
        row: PopupRow {
            name: workflow_connector_command_name(kind, &suggested_connection),
            description,
            replacement: command,
            append_space: false,
        },
        search_text: format!(
            "{} {} {}",
            workflow_connector_command_name(kind, &suggested_connection),
            slug,
            search_text
        )
        .to_ascii_lowercase(),
    }
}

fn workflow_connector_command(
    kind: WorkflowConnectorCommandKind,
    template: &ConnectorTemplate,
    suggested_connection: &str,
) -> String {
    match kind {
        WorkflowConnectorCommandKind::New => {
            format!("/workflows new {suggested_connection}-workflow {suggested_connection}")
        }
        WorkflowConnectorCommandKind::Append => format!(
            "/workflows append {suggested_connection} /tmp/{suggested_connection}.log --connector {}",
            template.slug
        ),
    }
}

fn workflow_connector_command_name(
    kind: WorkflowConnectorCommandKind,
    suggested_connection: &str,
) -> String {
    match kind {
        WorkflowConnectorCommandKind::New => format!("workflows new {suggested_connection}"),
        WorkflowConnectorCommandKind::Append => {
            format!("workflows append {suggested_connection}")
        }
    }
}

fn workflow_connector_command_description(
    kind: WorkflowConnectorCommandKind,
    template: &ConnectorTemplate,
    suggested_connection: &str,
) -> String {
    let action = match kind {
        WorkflowConnectorCommandKind::New => "Create draft",
        WorkflowConnectorCommandKind::Append => "Append events",
    };
    format!(
        "{action} from {}  connection={suggested_connection}; connector={}",
        template.description, template.slug
    )
}

fn workflow_connector_command_search_text(
    template: &ConnectorTemplate,
    suggested_connection: &str,
) -> String {
    let actions = template
        .actions
        .keys()
        .flat_map(|action| [action.to_string(), action.replace('_', " ")])
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} {} {} trigger trigger-ready event events workflow draft new append file save {}",
        template.slug,
        suggested_connection,
        template.description,
        template.skill,
        template.binary,
        actions
    )
    .to_ascii_lowercase()
}

fn monitor_connector_row(template: ConnectorTemplate) -> ConnectorPopupRow {
    let suggested_connection = suggested_connection_slug(&template.slug);
    let description = monitor_connector_description(&template, &suggested_connection);
    let search_text = monitor_connector_search_text(&template, &suggested_connection);
    ConnectorPopupRow {
        row: PopupRow {
            name: format!("monitor {suggested_connection}"),
            description,
            replacement: format!("/monitor {suggested_connection}"),
            append_space: false,
        },
        search_text,
    }
}

fn monitor_connector_description(
    template: &ConnectorTemplate,
    suggested_connection: &str,
) -> String {
    format!(
        "Monitor events from {}  connection={suggested_connection}; connector={}",
        template.description, template.slug
    )
}

fn monitor_connector_search_text(
    template: &ConnectorTemplate,
    suggested_connection: &str,
) -> String {
    let actions = template
        .actions
        .keys()
        .flat_map(|action| [action.to_string(), action.replace('_', " ")])
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} {} {} monitor monitoring task tasks triage trigger trigger-ready event events workflow background actionable {}",
        template.slug,
        suggested_connection,
        template.description,
        template.skill,
        template.binary,
        actions
    )
    .to_ascii_lowercase()
}

fn is_workflows_command(command: &str) -> bool {
    matches!(command, "workflow" | "workflows" | "pipeline" | "pipelines")
}

fn workflow_subcommand_matches(subcommand: &WorkflowSubcommand, filter: &str) -> bool {
    filter.is_empty()
        || subcommand.name.starts_with(filter)
        || subcommand.name.contains(filter)
        || subcommand.description.to_ascii_lowercase().contains(filter)
        || subcommand
            .hint
            .unwrap_or_default()
            .to_ascii_lowercase()
            .contains(filter)
        || subcommand
            .search_terms
            .iter()
            .any(|term| term.contains(filter))
}

fn workflow_subcommand_row(subcommand: &WorkflowSubcommand) -> PopupRow {
    PopupRow {
        name: format!("workflows {}", subcommand.name),
        description: subcommand
            .hint
            .map(|hint| format!("{}  {hint}", subcommand.description))
            .unwrap_or_else(|| subcommand.description.to_string()),
        replacement: format!("/workflows {}", subcommand.name),
        append_space: true,
    }
}

fn connector_catalog_rows(input: &str) -> Option<Vec<PopupRow>> {
    let rest = input
        .strip_prefix('/')?
        .split_once(' ')
        .filter(|(command, _)| *command == "connect")?
        .1;
    if connect_has_explicit_connection_name(rest) {
        return None;
    }
    let terms = search_terms(rest);
    let mut rows = live_connect_connection_rows()
        .into_iter()
        .filter(|row| terms.iter().all(|term| row.search_text.contains(term)))
        .collect::<Vec<_>>();
    rows.extend(
        builtin_connector_templates()
            .into_iter()
            .map(connector_popup_row)
            .filter(|row| terms.iter().all(|term| row.search_text.contains(term))),
    );
    rows.sort_by_key(|row| connector_row_sort_key(row, rest.trim()));
    rows.truncate(MAX_POPUP_ROWS);
    Some(rows.into_iter().map(|row| row.row).collect())
}

fn connect_has_explicit_connection_name(rest: &str) -> bool {
    let tokens = rest.split_whitespace().collect::<Vec<_>>();
    tokens.len() >= 2
        && builtin_connector_templates()
            .into_iter()
            .any(|template| template.slug == tokens[0])
}

fn connector_popup_row(template: ConnectorTemplate) -> ConnectorPopupRow {
    let suggested_connection = suggested_connection_slug(&template.slug);
    let slug = template.slug.clone();
    let description = connector_popup_description(&template, &suggested_connection);
    let search_text = connector_popup_search_text(&template, &suggested_connection);
    ConnectorPopupRow {
        row: PopupRow {
            name: format!("connect {slug}"),
            description,
            replacement: format!("/connect {slug} {suggested_connection}"),
            append_space: false,
        },
        search_text,
    }
}

fn connector_popup_description(template: &ConnectorTemplate, suggested_connection: &str) -> String {
    let mut details = vec![format!("connection={suggested_connection}")];
    details.push(
        if template.requires_auth {
            "auth"
        } else {
            "no auth"
        }
        .to_string(),
    );
    if template.can_subscribe {
        details.push("trigger".to_string());
    }
    if template.can_proxy_agent {
        details.push("agent proxy".to_string());
    }
    if !template.skill.trim().is_empty() {
        details.push(format!("skill={}", template.skill.trim()));
    }
    if !template.actions.is_empty() {
        let mut actions = template.actions.keys().cloned().collect::<Vec<_>>();
        actions.sort();
        details.push(format!("actions={}", actions.join(",")));
    }
    format!("{}  {}", template.description, details.join("; "))
}

fn connector_popup_search_text(template: &ConnectorTemplate, suggested_connection: &str) -> String {
    let actions = template
        .actions
        .keys()
        .flat_map(|action| [action.to_string(), action.replace('_', " ")])
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} {} {} {} {} {}",
        template.slug,
        suggested_connection,
        template.description,
        template.skill,
        template.binary,
        if template.requires_auth {
            "auth"
        } else {
            "no auth"
        },
        if template.can_subscribe {
            "trigger subscribe subscriber event events workflow"
        } else {
            "no trigger setup only"
        },
        actions
    )
    .to_ascii_lowercase()
}

fn connector_row_sort_key(row: &ConnectorPopupRow, filter: &str) -> (u8, String) {
    connector_command_row_sort_key(row, filter, "connect")
}

fn connector_command_row_sort_key(
    row: &ConnectorPopupRow,
    filter: &str,
    command: &str,
) -> (u8, String) {
    let name = row.row.name.to_ascii_lowercase();
    let description = row.row.description.to_ascii_lowercase();
    let filter = filter.to_ascii_lowercase();
    if filter.is_empty() {
        return (0, name);
    }
    if name == format!("{command} {filter}") || name == filter {
        return (0, name);
    }
    if name.starts_with(&format!("{command} {filter}")) || name.starts_with(&filter) {
        return (1, name);
    }
    if name.contains(&filter) {
        return (2, name);
    }
    if description.contains(&filter) {
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

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_core::{supported_commands, CommandKind, CommandSpec};

    #[test]
    fn popup_prefers_prefix_matches() {
        let commands = vec![
            visible_command("xreview"),
            visible_command("reflect"),
            visible_command("reload-plugins"),
            visible_command("feature"),
        ];
        let rows = popup_rows("/re", &commands);
        let names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();
        assert_eq!(names, ["reflect", "reload-plugins", "feature", "xreview"]);
    }

    #[test]
    fn popup_limits_broad_queries_to_eight_rows() {
        let commands = supported_commands();
        let rows = popup_rows("/", &commands);
        assert_eq!(rows.len(), 8);
    }

    #[test]
    fn popup_omits_hidden_commands() {
        let commands = vec![
            CommandSpec {
                name: "terminal-setup".to_string(),
                aliases: Vec::new(),
                description: "Install Shift+Enter".to_string(),
                argument_hint: None,
                kind: CommandKind::Local,
                hidden: true,
            },
            CommandSpec {
                name: "test".to_string(),
                aliases: Vec::new(),
                description: "Visible command".to_string(),
                argument_hint: None,
                kind: CommandKind::Local,
                hidden: false,
            },
        ];
        let rows = popup_rows("/te", &commands);

        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "test");
    }

    #[test]
    fn popup_matches_workflow_connector_terms_in_descriptions() {
        let commands = supported_commands();
        let rows = popup_rows("/connector", &commands);
        let names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();

        assert!(names.contains(&"connect"));
        assert!(names.contains(&"workflows"));
    }

    #[test]
    fn popup_matches_argument_hints() {
        let commands = supported_commands();
        let rows = popup_rows("/connector-slug", &commands);

        assert_eq!(rows.first().map(|row| row.name.as_str()), Some("connect"));
    }

    #[test]
    fn popup_matches_connect_catalog_after_command_space() {
        let commands = supported_commands();
        let rows = popup_rows("/connect tel", &commands);

        assert!(rows.iter().any(|row| row.name == "connect telegram-login"
            && row.replacement == "/connect telegram-login telegram-user"));
        assert!(rows.iter().any(|row| row.name == "connect telegram-bot"
            && row.replacement == "/connect telegram-bot telegram-bot"));
    }

    #[test]
    fn popup_matches_connect_catalog_metadata_terms() {
        let commands = supported_commands();
        let rows = popup_rows("/connect vote poll", &commands);

        assert!(rows.iter().any(|row| row.name == "connect telegram-login"));
        let event_rows = popup_rows("/connect imap events", &commands);
        assert!(event_rows.iter().any(|row| row.name == "connect email"));
    }

    #[test]
    fn popup_hides_connect_catalog_after_explicit_connection_name() {
        let commands = supported_commands();

        assert!(popup_rows("/connect telegram-login personal-account", &commands).is_empty());
        assert!(!super::popup_accepts_input(
            "/connect telegram-login personal-account"
        ));
    }

    #[test]
    fn popup_matches_workflow_new_connector_arguments() {
        let commands = supported_commands();
        let rows = popup_rows("/workflows new imap events", &commands);

        assert!(rows.iter().any(|row| row.name == "workflows new email"
            && row.replacement == "/workflows new email-workflow email"
            && row.description.contains("connection=email")));
        assert!(rows.iter().all(|row| row.name != "workflows new slack-app"));
    }

    #[test]
    fn popup_matches_workflow_append_connector_arguments() {
        let commands = supported_commands();
        let rows = popup_rows("/workflows append vote poll", &commands);

        assert!(rows.iter().any(|row| row.name == "workflows append telegram-user"
            && row.replacement
                == "/workflows append telegram-user /tmp/telegram-user.log --connector telegram-login"));
        assert!(rows
            .iter()
            .all(|row| row.name != "workflows append telegram-bot"));
    }

    #[test]
    fn popup_matches_monitor_connector_arguments() {
        let commands = supported_commands();
        let rows = popup_rows("/monitor vote poll", &commands);

        assert!(rows.iter().any(|row| row.name == "monitor telegram-user"
            && row.replacement == "/monitor telegram-user"
            && row.description.contains("connection=telegram-user")));
        assert!(rows.iter().all(|row| row.name != "monitor telegram-bot"));
        let event_rows = popup_rows("/monitor imap events", &commands);
        assert!(event_rows
            .iter()
            .any(|row| row.name == "monitor email" && row.replacement == "/monitor email"));
    }

    #[test]
    fn popup_accepts_monitor_search_terms() {
        assert!(super::popup_accepts_input("/monitor vote poll"));
    }

    #[test]
    fn popup_hides_workflow_connector_arguments_after_complete_command() {
        let commands = supported_commands();

        assert!(popup_rows("/workflows new email-workflow email", &commands).is_empty());
        assert!(popup_rows(
            "/workflows append email /tmp/email.log --connector email",
            &commands
        )
        .is_empty());
        assert!(!super::popup_accepts_input(
            "/workflows append email /tmp/email.log --connector email"
        ));
    }

    #[test]
    fn popup_matches_workflow_subcommands_after_command_space() {
        let commands = supported_commands();
        let rows = popup_rows("/workflows con", &commands);
        let names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();

        assert!(names.contains(&"workflows connections"));
        assert!(names.contains(&"workflows connectors"));
        assert!(rows
            .iter()
            .any(|row| row.replacement == "/workflows connectors"));
    }

    #[test]
    fn popup_hides_workflow_subcommands_after_query_space() {
        let commands = supported_commands();

        assert!(popup_rows("/workflows connectors telegram", &commands).is_empty());
        assert!(!super::popup_accepts_input(
            "/workflows connectors telegram"
        ));
    }

    fn visible_command(name: &str) -> CommandSpec {
        CommandSpec {
            name: name.to_string(),
            aliases: Vec::new(),
            description: "Visible command".to_string(),
            argument_hint: None,
            kind: CommandKind::Local,
            hidden: false,
        }
    }
}
