use super::{
    template_supports_event_workflow, workflow_connector_command_name, ConnectorPopupRow, PopupRow,
    WorkflowConnectorCommandKind,
};
use puffer_core::subscription_manager;
use puffer_subscriptions::{ConnectionRecord, ConnectorTemplate};
use std::collections::BTreeSet;

const APPEND_QUERY_STOP_WORDS: &[&str] = &[
    "append",
    "any",
    "containing",
    "contains",
    "event",
    "events",
    "file",
    "into",
    "match",
    "matching",
    "message",
    "messages",
    "on",
    "save",
    "that",
    "to",
    "where",
    "with",
    "workflow",
    "workflows",
];

/// Applies query-specific matching and command inference to workflow connector rows.
pub(super) struct WorkflowConnectorQuery {
    kind: WorkflowConnectorCommandKind,
    terms: Vec<String>,
    tokens: Vec<String>,
    append_path: Option<String>,
}

impl WorkflowConnectorQuery {
    /// Builds a workflow connector query helper for one slash popup render.
    pub(super) fn new(kind: WorkflowConnectorCommandKind, query: &str) -> Self {
        let tokens = query_tokens(query);
        let append_path = (kind == WorkflowConnectorCommandKind::Append)
            .then(|| append_query_path(&tokens))
            .flatten();
        Self {
            kind,
            terms: search_terms(query),
            tokens,
            append_path,
        }
    }

    /// Returns the row if it should be shown, with inferred append arguments applied.
    pub(super) fn prepare(&self, row: ConnectorPopupRow) -> Option<ConnectorPopupRow> {
        if let Some(intent) = self.append_intent_for_row(&row) {
            return Some(apply_append_intent(row, &intent));
        }
        self.terms
            .iter()
            .all(|term| row.search_text.contains(term))
            .then_some(row)
    }

    fn append_intent_for_row(&self, row: &ConnectorPopupRow) -> Option<AppendQueryIntent> {
        if self.kind != WorkflowConnectorCommandKind::Append {
            return None;
        }
        let path = self.append_path.as_ref()?;
        let identity = row_identity_terms(row);
        let names_row = self
            .tokens
            .iter()
            .map(|token| token.to_ascii_lowercase())
            .any(|token| identity.contains(&token));
        if !names_row {
            return None;
        }
        Some(AppendQueryIntent {
            path: path.clone(),
            pattern: append_query_pattern(&self.tokens, path, &identity),
        })
    }
}

struct AppendQueryIntent {
    path: String,
    pattern: Option<String>,
}

/// Returns workflow popup rows for configured event-capable connections.
pub(super) fn live_workflow_connection_rows(
    kind: WorkflowConnectorCommandKind,
) -> Vec<ConnectorPopupRow> {
    live_event_connection_rows()
        .into_iter()
        .map(|(connection, template)| workflow_connection_command_row(kind, connection, template))
        .collect()
}

/// Returns monitor popup rows for configured event-capable connections.
pub(super) fn live_monitor_connection_rows() -> Vec<ConnectorPopupRow> {
    live_event_connection_rows()
        .into_iter()
        .map(|(connection, template)| monitor_connection_row(connection, template))
        .collect()
}

/// Returns connect popup rows that repair configured connections.
pub(super) fn live_connect_connection_rows() -> Vec<ConnectorPopupRow> {
    live_configured_connection_rows()
        .into_iter()
        .map(|(connection, template)| connect_connection_row(connection, template))
        .collect()
}

fn live_event_connection_rows() -> Vec<(ConnectionRecord, ConnectorTemplate)> {
    live_configured_connection_rows()
        .into_iter()
        .filter(|(_, template)| template_supports_event_workflow(template))
        .collect()
}

fn live_configured_connection_rows() -> Vec<(ConnectionRecord, ConnectorTemplate)> {
    let Ok(manager) = subscription_manager() else {
        return Vec::new();
    };
    manager
        .connection_store()
        .list()
        .into_iter()
        .filter_map(|connection| {
            let template = manager.connector_store().get(&connection.connector_slug)?;
            Some((connection, template))
        })
        .collect()
}

fn workflow_connection_command_row(
    kind: WorkflowConnectorCommandKind,
    connection: ConnectionRecord,
    template: ConnectorTemplate,
) -> ConnectorPopupRow {
    let command = workflow_connection_command(kind, &connection);
    let description = workflow_connection_command_description(kind, &connection, &template);
    let search_text = live_connection_search_text(&connection, &template, &command);
    ConnectorPopupRow {
        row: PopupRow {
            name: workflow_connector_command_name(kind, &connection.slug),
            description,
            replacement: command,
            append_space: false,
        },
        search_text,
    }
}

fn apply_append_intent(
    mut row: ConnectorPopupRow,
    intent: &AppendQueryIntent,
) -> ConnectorPopupRow {
    let Some(connection) = append_row_connection_slug(&row) else {
        return row;
    };
    let connector_suffix = row
        .row
        .replacement
        .split_once(" --connector ")
        .map(|(_, connector)| format!(" --connector {connector}"))
        .unwrap_or_default();
    let path = quote_workflow_arg(&intent.path);
    let pattern = intent
        .pattern
        .as_deref()
        .map(|pattern| format!(" {}", quote_workflow_arg(pattern)))
        .unwrap_or_default();
    row.row.replacement =
        format!("/workflows append {connection} {path}{pattern}{connector_suffix}");
    row.search_text = format!("{} {}", row.search_text, row.row.replacement).to_ascii_lowercase();
    row
}

fn append_row_connection_slug(row: &ConnectorPopupRow) -> Option<String> {
    row.row
        .name
        .strip_prefix("workflows append ")
        .map(str::trim)
        .filter(|slug| !slug.is_empty())
        .map(str::to_string)
}

fn row_identity_terms(row: &ConnectorPopupRow) -> BTreeSet<String> {
    let mut terms = BTreeSet::new();
    if let Some(connection) = append_row_connection_slug(row) {
        terms.insert(connection.to_ascii_lowercase());
    }
    for token in row
        .row
        .description
        .split(|ch: char| ch.is_whitespace() || ch == ';')
    {
        if let Some(connector) = token.strip_prefix("connector=") {
            let connector = connector.trim_matches(|ch: char| ch == ',' || ch == '.');
            if !connector.is_empty() {
                terms.insert(connector.to_ascii_lowercase());
            }
        }
    }
    terms
}

fn append_query_path(tokens: &[String]) -> Option<String> {
    tokens
        .iter()
        .find(|token| looks_like_append_path(token) && append_path_valid(token))
        .cloned()
}

fn append_query_pattern(
    tokens: &[String],
    path: &str,
    identity: &BTreeSet<String>,
) -> Option<String> {
    let pattern = tokens
        .iter()
        .filter(|token| {
            let lower = token.to_ascii_lowercase();
            token.as_str() != path
                && !APPEND_QUERY_STOP_WORDS.contains(&lower.as_str())
                && !identity.contains(&lower)
                && !looks_like_append_path(token)
        })
        .cloned()
        .collect::<Vec<_>>()
        .join(" ");
    (!pattern.trim().is_empty()).then_some(pattern)
}

fn query_tokens(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(strip_query_token_quotes)
        .filter(|token| !token.is_empty())
        .collect()
}

fn strip_query_token_quotes(value: &str) -> String {
    if value.len() >= 2
        && ((value.starts_with('\'') && value.ends_with('\''))
            || (value.starts_with('"') && value.ends_with('"')))
    {
        return value[1..value.len() - 1].to_string();
    }
    value.to_string()
}

fn looks_like_append_path(value: &str) -> bool {
    value.starts_with('/')
        || value.starts_with("./")
        || value.starts_with("../")
        || value.contains('/')
        || value.contains('.')
}

fn append_path_valid(value: &str) -> bool {
    let path = value.trim();
    let segments = path.split('/').filter(|segment| !segment.is_empty());
    !path.is_empty()
        && !path.starts_with("~/")
        && !segments.into_iter().any(|segment| segment == "..")
        && (!path.starts_with('/') || path.starts_with("/tmp/"))
}

fn search_terms(query: &str) -> Vec<String> {
    query
        .split_whitespace()
        .map(str::trim)
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

fn quote_workflow_arg(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || "_./:@%+=,-".contains(ch))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn workflow_connection_command(
    kind: WorkflowConnectorCommandKind,
    connection: &ConnectionRecord,
) -> String {
    match kind {
        WorkflowConnectorCommandKind::New => {
            format!(
                "/workflows new {}-workflow {}",
                connection.slug, connection.slug
            )
        }
        WorkflowConnectorCommandKind::Append => {
            format!(
                "/workflows append {} /tmp/{}.log",
                connection.slug, connection.slug
            )
        }
    }
}

fn workflow_connection_command_description(
    kind: WorkflowConnectorCommandKind,
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
) -> String {
    let action = match kind {
        WorkflowConnectorCommandKind::New => "Create draft workflow",
        WorkflowConnectorCommandKind::Append => "Append events to a file",
    };
    format!(
        "{action} from configured {}; connection={}; connector={}",
        template.description, connection.slug, connection.connector_slug
    )
}

fn monitor_connection_row(
    connection: ConnectionRecord,
    template: ConnectorTemplate,
) -> ConnectorPopupRow {
    let command = format!("/monitor {}", connection.slug);
    let search_text = live_connection_search_text(&connection, &template, &command);
    ConnectorPopupRow {
        row: PopupRow {
            name: format!("monitor {}", connection.slug),
            description: format!(
                "Create monitor workflow for configured {}; connection={}; connector={}",
                template.description, connection.slug, connection.connector_slug
            ),
            replacement: command,
            append_space: false,
        },
        search_text,
    }
}

fn connect_connection_row(
    connection: ConnectionRecord,
    template: ConnectorTemplate,
) -> ConnectorPopupRow {
    let replacement = format!("/connect {} {}", connection.connector_slug, connection.slug);
    let search_text = live_connection_search_text(&connection, &template, &replacement);
    ConnectorPopupRow {
        row: PopupRow {
            name: format!("connect {}", connection.slug),
            description: format!(
                "Repair connector setup for configured {}; connection={}; connector={}",
                template.description, connection.slug, connection.connector_slug
            ),
            replacement,
            append_space: false,
        },
        search_text,
    }
}

fn live_connection_search_text(
    connection: &ConnectionRecord,
    template: &ConnectorTemplate,
    command: &str,
) -> String {
    let actions = template
        .actions
        .keys()
        .flat_map(|action| [action.to_string(), action.replace('_', " ")])
        .collect::<Vec<_>>()
        .join(" ");
    format!(
        "{} {} {} {} {:?} {} {} {} {} trigger trigger-ready event events workflow draft new append file save monitor repair reconnect configured existing {}",
        connection.slug,
        connection.connector_slug,
        connection.description,
        template.description,
        connection.state,
        template.skill,
        template.binary,
        command,
        if connection.has_consumer { "consumer active" } else { "consumer idle" },
        actions
    )
    .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_subscriptions::{builtin_connector_template, ConnectionRecord};

    #[test]
    fn workflow_connection_rows_use_configured_connection_slugs() {
        let template = builtin_connector_template("telegram-login").unwrap();
        let connection =
            ConnectionRecord::authenticated("work-telegram", "telegram-login", "Work Telegram");

        let row = workflow_connection_command_row(
            WorkflowConnectorCommandKind::Append,
            connection,
            template,
        );

        assert_eq!(row.row.name, "workflows append work-telegram");
        assert_eq!(
            row.row.replacement,
            "/workflows append work-telegram /tmp/work-telegram.log"
        );
        assert!(row.row.description.contains("configured"));
        assert!(row.search_text.contains("work-telegram"));
        assert!(row.search_text.contains("telegram-login"));
    }

    #[test]
    fn append_query_intent_uses_search_path_and_pattern() {
        let template = builtin_connector_template("telegram-login").unwrap();
        let row = workflow_connection_command_row(
            WorkflowConnectorCommandKind::Append,
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Telegram"),
            template,
        );
        let query = WorkflowConnectorQuery::new(
            WorkflowConnectorCommandKind::Append,
            "telegram-user support ping /tmp/support",
        );

        let row = query.prepare(row).expect("matching row");

        assert_eq!(
            row.row.replacement,
            "/workflows append telegram-user /tmp/support 'support ping'"
        );
    }

    #[test]
    fn append_query_intent_keeps_broad_path_queries_narrow() {
        let template = builtin_connector_template("telegram-login").unwrap();
        let row = workflow_connection_command_row(
            WorkflowConnectorCommandKind::Append,
            ConnectionRecord::authenticated("telegram-user", "telegram-login", "Telegram"),
            template,
        );
        let query =
            WorkflowConnectorQuery::new(WorkflowConnectorCommandKind::Append, "/tmp/support");

        assert!(query.prepare(row).is_none());
    }

    #[test]
    fn monitor_connection_rows_use_configured_connection_slugs() {
        let template = builtin_connector_template("telegram-login").unwrap();
        let connection =
            ConnectionRecord::authenticated("work-telegram", "telegram-login", "Work Telegram");

        let row = monitor_connection_row(connection, template);

        assert_eq!(row.row.name, "monitor work-telegram");
        assert_eq!(row.row.replacement, "/monitor work-telegram");
        assert!(row.row.description.contains("configured"));
        assert!(row.search_text.contains("monitor"));
        assert!(row.search_text.contains("work telegram"));
    }

    #[test]
    fn connect_connection_rows_repair_configured_connection_slugs() {
        let template = builtin_connector_template("telegram-login").unwrap();
        let connection =
            ConnectionRecord::authenticated("work-telegram", "telegram-login", "Work Telegram");

        let row = connect_connection_row(connection, template);

        assert_eq!(row.row.name, "connect work-telegram");
        assert_eq!(row.row.replacement, "/connect telegram-login work-telegram");
        assert!(row.row.description.contains("Repair connector setup"));
        assert!(row.search_text.contains("repair"));
    }
}
