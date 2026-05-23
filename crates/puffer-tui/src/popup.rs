use puffer_core::CommandSpec;

const MAX_POPUP_ROWS: usize = 8;

/// Returns slash-command popup rows for the current slash-input prefix.
pub(crate) fn popup_rows<'a>(input: &str, commands: &'a [CommandSpec]) -> Vec<&'a CommandSpec> {
    let filter = input.trim_start_matches('/');
    let mut rows = commands
        .iter()
        .filter(|command| !command.hidden)
        .filter(|command| command_matches(command, filter))
        .collect::<Vec<_>>();
    rows.sort_by_key(|command| sort_key(command, filter));
    rows.truncate(MAX_POPUP_ROWS);
    rows
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
}

fn sort_key(command: &CommandSpec, filter: &str) -> (u8, String) {
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
    (3, command.name.to_string())
}

#[cfg(test)]
mod tests {
    use super::popup_rows;
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
