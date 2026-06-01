use puffer_config::ConfigPaths;
use puffer_core::{AppState, CommandActionEntry, MessageRole};

pub(crate) fn open_command_picker(
    tui: &mut crate::TuiState,
    title: &str,
    entries: Vec<crate::ModelPickerEntry>,
) -> bool {
    if entries.is_empty() {
        return false;
    }
    crate::flow::set_overlay_state(
        tui,
        Some(crate::OverlayState::CommandPicker {
            title: title.to_string(),
            entries,
            selection: 0,
        }),
    );
    true
}

pub(crate) fn command_picker_entries(
    actions: impl IntoIterator<Item = CommandActionEntry>,
) -> Vec<crate::ModelPickerEntry> {
    actions
        .into_iter()
        .map(|entry| crate::ModelPickerEntry {
            selector: entry.command.clone(),
            description: entry.description,
            command: Some(entry.command),
        })
        .collect()
}

pub(crate) fn open_tag_confirmation_picker(
    state: &AppState,
    tui: &mut crate::TuiState,
    args: &str,
) -> bool {
    let tag = args.trim();
    if tag.is_empty()
        || matches!(
            tag,
            "help"
                | "-h"
                | "--help"
                | "list"
                | "show"
                | "display"
                | "current"
                | "view"
                | "get"
                | "check"
                | "describe"
                | "print"
                | "version"
                | "about"
                | "status"
                | "?"
        )
        || !state.session.tags.iter().any(|existing| existing == tag)
    {
        return false;
    }

    crate::flow::set_overlay_state(
        tui,
        Some(crate::OverlayState::CommandPicker {
            title: "Remove Tag?".to_string(),
            entries: vec![
                crate::ModelPickerEntry {
                    selector: "Yes, remove tag".to_string(),
                    description: format!("Current tag: #{tag}"),
                    command: Some(format!("/tag --confirm-remove {tag}")),
                },
                crate::ModelPickerEntry {
                    selector: "No, keep tag".to_string(),
                    description: String::new(),
                    command: Some(format!("/tag --keep {tag}")),
                },
            ],
            selection: 0,
        }),
    );
    true
}

pub(crate) fn rewind_picker_entries(state: &AppState) -> Vec<crate::ModelPickerEntry> {
    let mut entries = vec![crate::ModelPickerEntry {
        selector: "/rewind".to_string(),
        description: "Remove the latest rendered transcript item".to_string(),
        command: None,
    }];
    entries.extend(
        state
            .transcript
            .iter()
            .filter(|message| message.role == MessageRole::User)
            .enumerate()
            .map(|(index, message)| crate::ModelPickerEntry {
                selector: format!("/rewind {}", index + 1),
                description: truncate_rewind_label(&message.text),
                command: None,
            }),
    );
    entries
}

fn truncate_rewind_label(text: &str) -> String {
    let line = text
        .lines()
        .find(|line| !line.trim().is_empty())
        .map(str::trim)
        .unwrap_or("<empty>");
    if line.chars().count() <= 60 {
        line.to_string()
    } else {
        format!("{}...", line.chars().take(57).collect::<String>())
    }
}

pub(crate) fn memory_picker_entries(state: &AppState) -> Vec<crate::ModelPickerEntry> {
    let paths = ConfigPaths::discover(&state.cwd);
    let project_description = state
        .project_memory
        .as_ref()
        .map(|context| {
            format!(
                "{} ({})",
                context.memory_file.display(),
                if context.memory_file.exists() {
                    "present"
                } else {
                    "new"
                }
            )
        })
        .unwrap_or_else(|| "project MEMORY.md (will initialize for current directory)".to_string());
    let fixed_entries = [
        ("workspace", paths.workspace_config_dir.join("memory.md")),
        ("user", paths.user_config_dir.join("memory.md")),
    ];
    let mut entries = vec![crate::ModelPickerEntry {
        selector: "/memory open project".to_string(),
        description: project_description,
        command: None,
    }];
    entries.extend(
        fixed_entries
            .into_iter()
            .map(|(scope, path)| crate::ModelPickerEntry {
                selector: format!("/memory open {scope}"),
                description: format!(
                    "{} ({})",
                    path.display(),
                    if path.exists() { "present" } else { "new" }
                ),
                command: None,
            }),
    );
    entries
}
