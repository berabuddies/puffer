//! Internal CLI-only surfaces for third-party tools.

use crate::browser;
use crate::browser_args::BrowserArgs;
use crate::cli_args::InternalToolCommand;
use crate::subscriber_tools;
use anyhow::Result;
use puffer_config::ConfigPaths;
use puffer_tools::internal_tools::internal_cli_tools;
use std::path::Path;

/// Runs an internal third-party tool command.
pub(crate) fn run_internal_tool_command(
    cwd: &Path,
    paths: &ConfigPaths,
    command: InternalToolCommand,
) -> Result<()> {
    match command {
        InternalToolCommand::Aliases => print_alias_setup(),
        InternalToolCommand::Browser(args) => run_browser(cwd, paths, args),
        InternalToolCommand::Email(args) => subscriber_tools::run_email(args),
        InternalToolCommand::Lark(args) => subscriber_tools::run_lark(args),
        InternalToolCommand::Slack(args) => subscriber_tools::run_slack(args),
        InternalToolCommand::Telegram(args) => subscriber_tools::run_telegram(args),
    }
}

fn run_browser(cwd: &Path, paths: &ConfigPaths, args: BrowserArgs) -> Result<()> {
    browser::run_internal_browser_command(cwd, paths, args)
}

fn print_alias_setup() -> Result<()> {
    for tool in internal_cli_tools() {
        for alias in tool.aliases {
            println!(
                "alias {alias}='puffer internal-tool {}'",
                shell_quote(tool.id)
            );
        }
        println!("# skill: {}", tool.skill_name);
    }
    Ok(())
}

fn shell_quote(value: &str) -> String {
    if value
        .chars()
        .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.'))
    {
        return value.to_string();
    }
    format!("{value:?}")
}
