//! Internal CLI-only surfaces for third-party tools.

use crate::browser;
use crate::browser_args::BrowserArgs;
use crate::cli_args::InternalToolCommand;
use anyhow::Result;
use puffer_config::ConfigPaths;
use std::path::Path;

/// Describes one third-party tool that is intentionally not model-visible.
pub(crate) trait ThreePpTool {
    /// Returns the stable internal tool id.
    fn id(&self) -> &'static str;

    /// Returns shell aliases that should map to `puffer internal-tool`.
    fn aliases(&self) -> &'static [&'static str];

    /// Returns the skill resource name that teaches agents how to use the CLI.
    fn skill_name(&self) -> &'static str;
}

struct BrowserThreePpTool;

impl ThreePpTool for BrowserThreePpTool {
    fn id(&self) -> &'static str {
        "browser"
    }

    fn aliases(&self) -> &'static [&'static str] {
        &["browser"]
    }

    fn skill_name(&self) -> &'static str {
        "browser"
    }
}

static BROWSER_TOOL: BrowserThreePpTool = BrowserThreePpTool;

/// Runs an internal third-party tool command.
pub(crate) fn run_internal_tool_command(
    cwd: &Path,
    paths: &ConfigPaths,
    command: InternalToolCommand,
) -> Result<()> {
    match command {
        InternalToolCommand::Aliases => print_alias_setup(),
        InternalToolCommand::Browser(args) => run_browser(cwd, paths, args),
    }
}

fn run_browser(cwd: &Path, paths: &ConfigPaths, args: BrowserArgs) -> Result<()> {
    browser::run_internal_browser_command(cwd, paths, args)
}

fn print_alias_setup() -> Result<()> {
    for tool in three_pp_tools() {
        for alias in tool.aliases() {
            println!(
                "alias {alias}='puffer internal-tool {}'",
                shell_quote(tool.id())
            );
        }
        println!("# skill: {}", tool.skill_name());
    }
    Ok(())
}

fn three_pp_tools() -> [&'static dyn ThreePpTool; 1] {
    [&BROWSER_TOOL]
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
