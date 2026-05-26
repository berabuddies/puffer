use super::{
    template_supports_event_workflow, workflow_connector_command_name, ConnectorPopupRow, PopupRow,
    WorkflowConnectorCommandKind,
};
use puffer_core::subscription_manager;
use puffer_subscriptions::{ConnectionRecord, ConnectorTemplate};

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
        WorkflowConnectorCommandKind::New => "Create draft",
        WorkflowConnectorCommandKind::Append => "Append events",
    };
    format!(
        "{action} from configured {}  connection={}; connector={}",
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
                "Monitor events from configured {}  connection={}; connector={}",
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
                "Repair configured {}  connection={}; connector={}",
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
        assert!(row.row.description.contains("Repair configured"));
        assert!(row.search_text.contains("repair"));
    }
}
