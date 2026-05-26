use super::{PopupRow, MAX_POPUP_ROWS};

struct WorkflowSubcommand {
    name: &'static str,
    description: &'static str,
    hint: Option<&'static str>,
    search_terms: &'static [&'static str],
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
        name: "pause",
        description: "Pause an enabled workflow action binding",
        hint: Some("<binding-slug>"),
        search_terms: &["disable", "stop", "binding", "action"],
    },
    WorkflowSubcommand {
        name: "resume",
        description: "Resume a paused workflow action binding",
        hint: Some("<binding-slug>"),
        search_terms: &["enable", "start", "unpause", "binding", "action"],
    },
    WorkflowSubcommand {
        name: "delete",
        description: "Remove a workflow action binding",
        hint: Some("<binding-slug>"),
        search_terms: &["remove", "rm", "cleanup", "binding", "action"],
    },
    WorkflowSubcommand {
        name: "actions",
        description: "Search workflow action bindings and management commands",
        hint: Some("[query]"),
        search_terms: &[
            "bindings", "append", "file", "pause", "resume", "delete", "pattern",
        ],
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

/// Returns workflow subcommand completion rows for `/workflows ...`.
pub(super) fn workflow_subcommand_rows(input: &str) -> Option<Vec<PopupRow>> {
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

fn is_workflows_command(command: &str) -> bool {
    matches!(command, "workflow" | "workflows" | "pipeline" | "pipelines")
}
