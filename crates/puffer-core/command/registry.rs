use crate::command_helpers::{
    should_hide_terminal_setup_command, terminal_setup_command_description,
};
use puffer_resources::LoadedResources;
use serde::Serialize;
use std::collections::BTreeSet;

/// Distinguishes prompt-backed, local, and UI-oriented slash commands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum CommandKind {
    Prompt,
    Local,
    Ui,
}

/// Declares one built-in slash command supported by Puffer.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CommandSpec {
    pub name: String,
    pub aliases: Vec<String>,
    pub description: String,
    pub argument_hint: Option<String>,
    pub kind: CommandKind,
    pub hidden: bool,
}

/// Returns the full built-in slash-command surface supported by Puffer.
pub fn supported_commands() -> Vec<CommandSpec> {
    vec![
        cmd(
            "add-dir",
            &[],
            "Add a new working directory",
            Some("<path>"),
            CommandKind::Local,
        ),
        cmd(
            "agents",
            &[],
            "Manage agent configurations",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "autodream",
            &["dream"],
            "Consolidate durable project memory and suggest skill-worthy traces",
            Some("[status]"),
            CommandKind::Local,
        ),
        cmd(
            "branch",
            &["fork"],
            "Create a branch of the current conversation at this point",
            Some("[name]"),
            CommandKind::Local,
        ),
        cmd(
            "btw",
            &[],
            "Ask a quick side question without interrupting the main conversation",
            Some("<question>"),
            CommandKind::Local,
        ),
        cmd(
            "buddy",
            &[],
            "Show or interact with Clawd",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "clear",
            &["reset", "new"],
            "Clear conversation history and free up context",
            None,
            CommandKind::Local,
        ),
        cmd(
            "color",
            &[],
            "Set the prompt bar color for this session",
            Some("<color|default>"),
            CommandKind::Ui,
        ),
        cmd(
            "commit",
            &[],
            "Create a git commit",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "compact",
            &[],
            "Summarize the conversation to preserve context budget",
            Some("<optional custom summarization instructions>"),
            CommandKind::Prompt,
        ),
        cmd(
            "config",
            &["settings"],
            "Open config panel",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "connect",
            &[],
            "Configure a named connector connection",
            Some("<connector-slug> <connection-name>"),
            CommandKind::Local,
        ),
        cmd(
            "context",
            &[],
            "Show current context usage",
            None,
            CommandKind::Local,
        ),
        cmd(
            "copy",
            &[],
            "Copy the latest assistant output",
            None,
            CommandKind::Local,
        ),
        cmd(
            "cost",
            &[],
            "Show the total cost and duration of the current session",
            None,
            CommandKind::Local,
        ),
        cmd(
            "diff",
            &[],
            "View uncommitted changes and per-turn diffs",
            None,
            CommandKind::Local,
        ),
        cmd(
            "debug",
            &[],
            "Show the full raw context sent to the model",
            None,
            CommandKind::Local,
        ),
        cmd(
            "doctor",
            &[],
            "Diagnose and verify your Puffer Code installation and settings",
            None,
            CommandKind::Local,
        ),
        cmd(
            "effort",
            &[],
            "Set effort level for model usage",
            Some("[minimal|low|medium|high|xhigh|max|auto]"),
            CommandKind::Ui,
        ),
        cmd("exit", &["quit"], "Exit the REPL", None, CommandKind::Local),
        cmd(
            "export",
            &[],
            "Export the current conversation to a file or clipboard",
            Some("[filename]"),
            CommandKind::Local,
        ),
        cmd(
            "files",
            &[],
            "List all files currently in context",
            None,
            CommandKind::Local,
        ),
        cmd(
            "fast",
            &[],
            "Toggle fast mode",
            Some("[on|off]"),
            CommandKind::Ui,
        ),
        cmd(
            "goal",
            &["objective"],
            "Set or view the goal for a long-running task",
            Some("[<text> | clear | budget <N> | status]"),
            CommandKind::Local,
        ),
        cmd(
            "genskill",
            &[],
            "Generate a reusable skill from the current conversation",
            Some("[--candidates N] [--rounds K]"),
            CommandKind::Local,
        ),
        cmd(
            "help",
            &["?"],
            "Show help and available commands",
            None,
            CommandKind::Local,
        ),
        cmd(
            "hooks",
            &[],
            "View hook configurations for tool events",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "ide",
            &[],
            "Manage IDE integrations and show status",
            Some("[open]"),
            CommandKind::Ui,
        ),
        cmd(
            "init",
            &[],
            "Initialize project guidance and defaults",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "keybindings",
            &[],
            "Open or create your keybindings configuration file",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "login",
            &[],
            "Sign in to a provider",
            Some("[provider]"),
            CommandKind::Ui,
        ),
        cmd(
            "loop",
            &[],
            "Run a prompt on a recurring interval until a condition is met",
            Some("<interval> <prompt> | stop"),
            CommandKind::Local,
        ),
        cmd(
            "logout",
            &[],
            "Sign out from a provider",
            Some("[provider]"),
            CommandKind::Ui,
        ),
        cmd(
            "maximize",
            &["max"],
            "Iteratively maximize a metric via repeated prompt execution",
            Some("<metric> <prompt>"),
            CommandKind::Local,
        ),
        cmd(
            "mcp",
            &[],
            "Manage MCP servers",
            Some("[enable|disable [server-name]]"),
            CommandKind::Ui,
        ),
        cmd(
            "minimize",
            &["min"],
            "Iteratively minimize a metric via repeated prompt execution",
            Some("<metric> <prompt>"),
            CommandKind::Local,
        ),
        cmd("memory", &[], "Edit memory files", None, CommandKind::Ui),
        cmd(
            "recap",
            &[],
            "Summarize the session in 1-2 sentences (transient, not persisted)",
            None,
            CommandKind::Local,
        ),
        cmd(
            "model",
            &[],
            "Select the active model",
            Some("[provider/model]"),
            CommandKind::Ui,
        ),
        cmd(
            "monitor",
            &[],
            "Create connector monitors that turn messages into tasks",
            Some("[connection search | connection ...]"),
            CommandKind::Local,
        ),
        cmd(
            "pentest",
            &[],
            "Run an authorized web penetration test",
            Some("<url> [iter] [disp] [task-spec=<file>]"),
            CommandKind::Local,
        ),
        cmd(
            "permissions",
            &["allowed-tools"],
            "Manage allow & deny tool permission rules",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "plan",
            &[],
            "Enable plan mode or view the current session plan",
            Some("[open|<description>]"),
            CommandKind::Local,
        ),
        cmd(
            "plugin",
            &["plugins", "marketplace"],
            "Manage Puffer plugins",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "pr-comments",
            &[],
            "Get comments from a GitHub pull request",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "reflect",
            &[],
            "Toggle runtime reflection (stall/loop detection) for this session",
            Some("[on|off|toggle|status|lang <zh|en>|mode <confirm|independent>|llm <on|off>]"),
            CommandKind::Local,
        ),
        cmd(
            "reload-plugins",
            &[],
            "Activate pending plugin changes in the current session",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "remote-control",
            &["rc"],
            "Connect this terminal for remote-control sessions",
            Some("[name]"),
            CommandKind::Ui,
        ),
        cmd(
            "remote-env",
            &[],
            "Configure the remote environment for this session",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "rename",
            &[],
            "Rename the current conversation",
            Some("[name]"),
            CommandKind::Local,
        ),
        cmd(
            "resume",
            &["continue"],
            "Resume a previous conversation",
            Some("[conversation id or search term]"),
            CommandKind::Ui,
        ),
        cmd(
            "rewind",
            &["checkpoint"],
            "Restore the code and/or conversation to a previous point",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "review",
            &[],
            "Review the current worktree or pull request",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "sandbox",
            &[],
            "Explain that sandbox mode has been removed",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "security-review",
            &[],
            "Complete a security review of the pending changes on the current branch",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "session",
            &["remote"],
            "Show remote session URL and QR code",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "skills",
            &[],
            "List available skills",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "skill:<name>",
            &[],
            "Run a loaded skill by slash-safe skill name",
            None,
            CommandKind::Ui,
        ),
        cmd(
            "status",
            &[],
            "Show current session configuration and status",
            None,
            CommandKind::Local,
        ),
        cmd(
            "statusline",
            &[],
            "Set up Claude Code's status line UI",
            None,
            CommandKind::Prompt,
        ),
        cmd(
            "tag",
            &[],
            "Toggle a searchable tag on the current session",
            Some("<tag-name>"),
            CommandKind::Ui,
        ),
        cmd(
            "tasks",
            &["bashes"],
            "List and manage background tasks",
            None,
            CommandKind::Ui,
        ),
        cmd_hidden(
            "terminal-setup",
            &[],
            terminal_setup_command_description(),
            None,
            CommandKind::Local,
            should_hide_terminal_setup_command(),
        ),
        cmd("theme", &[], "Change the theme", None, CommandKind::Local),
        cmd(
            "ultrareview",
            &[],
            "Multi-agent code review of the current worktree or PR",
            Some("[pr-url-or-number]"),
            CommandKind::Local,
        ),
        cmd(
            "usage",
            &[],
            "Show plan usage limits",
            None,
            CommandKind::Local,
        ),
        cmd(
            "vim",
            &[],
            "Toggle between Vim and Normal editing modes",
            None,
            CommandKind::Local,
        ),
        cmd(
            "workflows",
            &["workflow"],
            "Show workflow, connector, and connection status",
            Some("[list|new|append|delete|actions|connections|connectors|tasks|runs] [query]"),
            CommandKind::Local,
        ),
    ]
}

/// Returns the full slash-command surface, including loaded user-invocable skills.
pub fn command_surface(resources: &LoadedResources) -> Vec<CommandSpec> {
    let mut commands = supported_commands();
    let reserved_names = commands
        .iter()
        .flat_map(|command| {
            std::iter::once(command.name.as_str()).chain(command.aliases.iter().map(String::as_str))
        })
        .collect::<BTreeSet<_>>();
    let mut skills = resources
        .skills
        .iter()
        .filter(|skill| skill.value.user_invocable)
        .filter(|skill| !reserved_names.contains(skill.value.name.as_str()))
        .map(|skill| CommandSpec {
            name: skill.value.name.clone(),
            aliases: vec![format!("skill:{}", skill.value.name)],
            description: skill.value.description.clone(),
            argument_hint: skill.value.argument_hint.clone(),
            kind: CommandKind::Prompt,
            hidden: false,
        })
        .collect::<Vec<_>>();
    skills.sort_by(|left, right| left.name.cmp(&right.name));
    commands.extend(skills);
    commands
}

/// Finds a command by canonical name or alias.
pub fn find_command<'a>(commands: &'a [CommandSpec], name: &str) -> Option<&'a CommandSpec> {
    commands
        .iter()
        .find(|command| command.name == name || command.aliases.iter().any(|alias| alias == name))
}

fn cmd(
    name: &'static str,
    aliases: &'static [&'static str],
    description: impl Into<String>,
    argument_hint: Option<&'static str>,
    kind: CommandKind,
) -> CommandSpec {
    cmd_hidden(name, aliases, description, argument_hint, kind, false)
}

fn cmd_hidden(
    name: &'static str,
    aliases: &'static [&'static str],
    description: impl Into<String>,
    argument_hint: Option<&'static str>,
    kind: CommandKind,
    hidden: bool,
) -> CommandSpec {
    CommandSpec {
        name: name.to_string(),
        aliases: aliases.iter().map(|alias| (*alias).to_string()).collect(),
        description: description.into(),
        argument_hint: argument_hint.map(str::to_string),
        kind,
        hidden,
    }
}
