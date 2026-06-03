//! Shared descriptors for first-party CLI-only internal tools.

/// Static metadata for one internal CLI tool.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InternalCliToolDescriptor {
    /// Stable internal tool id used by `puffer internal-tool <id>`.
    pub id: &'static str,
    /// Shell helper names that should invoke the internal tool.
    pub aliases: &'static [&'static str],
    /// Skill resource name that documents this internal tool for agents.
    pub skill_name: &'static str,
}

const BROWSER_ALIASES: &[&str] = &["browser"];
const EMAIL_ALIASES: &[&str] = &["email"];
const SLACK_ALIASES: &[&str] = &["slack"];
const TELEGRAM_ALIASES: &[&str] = &["telegram"];

const INTERNAL_CLI_TOOLS: &[InternalCliToolDescriptor] = &[
    InternalCliToolDescriptor {
        id: "browser",
        aliases: BROWSER_ALIASES,
        skill_name: "browser",
    },
    InternalCliToolDescriptor {
        id: "email",
        aliases: EMAIL_ALIASES,
        skill_name: "email",
    },
    InternalCliToolDescriptor {
        id: "slack",
        aliases: SLACK_ALIASES,
        skill_name: "slack",
    },
    InternalCliToolDescriptor {
        id: "telegram",
        aliases: TELEGRAM_ALIASES,
        skill_name: "telegram",
    },
];

/// Returns all CLI-only internal tool descriptors.
pub fn internal_cli_tools() -> &'static [InternalCliToolDescriptor] {
    INTERNAL_CLI_TOOLS
}

/// Renders shell functions for all known internal CLI tool aliases.
pub fn internal_tool_shell_helpers(executable: &str) -> String {
    internal_cli_tools()
        .iter()
        .flat_map(|tool| {
            tool.aliases
                .iter()
                .map(move |alias| internal_tool_shell_function(executable, alias, tool.id))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn internal_tool_shell_function(executable: &str, alias: &str, tool_id: &str) -> String {
    format!(
        "{alias}() {{\n  {} internal-tool {} \"$@\"\n}}",
        shell_word(executable),
        shell_word(tool_id)
    )
}

fn shell_word(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn internal_tool_shell_helpers_include_browser() {
        let helpers = internal_tool_shell_helpers("/tmp/puffer");

        assert!(helpers.contains("browser()"));
        assert!(helpers.contains("'/tmp/puffer' internal-tool 'browser' \"$@\""));
        assert!(helpers.contains("email()"));
        assert!(helpers.contains("'/tmp/puffer' internal-tool 'email' \"$@\""));
        assert!(helpers.contains("slack()"));
        assert!(helpers.contains("'/tmp/puffer' internal-tool 'slack' \"$@\""));
        assert!(helpers.contains("telegram()"));
        assert!(helpers.contains("'/tmp/puffer' internal-tool 'telegram' \"$@\""));
    }

    #[test]
    fn internal_tool_shell_helpers_quote_executable() {
        let helpers = internal_tool_shell_helpers("/tmp/puffer's/bin/puffer");

        assert!(helpers.contains("'/tmp/puffer'\"'\"'s/bin/puffer'"));
    }
}
