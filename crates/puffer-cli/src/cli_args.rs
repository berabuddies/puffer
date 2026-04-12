use clap::{Parser, Subcommand, ValueEnum};

#[derive(Debug, Parser)]
#[command(
    bin_name = "puffer",
    subcommand_negates_reqs = true,
    args_conflicts_with_subcommands = true
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    pub(crate) subcommand: Option<Command>,

    /// Resume a conversation by session ID or search term, or open the picker when omitted.
    #[arg(
        short = 'r',
        long = "resume",
        num_args = 0..=1,
        default_missing_value = "",
        value_name = "SESSION"
    )]
    pub(crate) resume: Option<String>,

    /// Optional prompt to submit immediately when launching the TUI.
    pub(crate) prompt: Option<String>,

    /// Disable alternate-screen mode for the TUI.
    #[arg(long = "no-alt-screen", default_value_t = false)]
    pub(crate) no_alt_screen: bool,
}

#[derive(Debug, Subcommand)]
pub(crate) enum Command {
    /// List configured agents.
    Agents {
        /// Accepted for Claude compatibility; Puffer maps user to user resources and project/local to workspace resources.
        #[arg(long = "setting-sources")]
        setting_sources: Option<String>,
    },
    /// Manage stored provider credentials.
    Auth {
        #[command(subcommand)]
        command: AuthCommand,
    },
    /// Inspect Puffer auto-mode compatibility state.
    AutoMode {
        #[command(subcommand)]
        command: Option<AutoModeCommand>,
    },
    /// Diagnose the current Puffer installation and workspace.
    Doctor,
    /// Print source install instructions for this build.
    Install {
        /// Optional install target label for compatibility with Claude Code.
        target: Option<String>,
        /// Include `--force` in the suggested install command.
        #[arg(long = "force", default_value_t = false)]
        force: bool,
    },
    /// Configure and manage MCP servers.
    Mcp {
        #[command(subcommand)]
        command: Option<McpCommand>,
    },
    /// Manage Puffer plugins.
    #[command(visible_alias = "plugins")]
    Plugin {
        #[command(subcommand)]
        command: Option<PluginCommand>,
    },
    /// Store a long-lived API token for a provider.
    SetupToken {
        /// Provider id. Defaults to the configured provider or `anthropic`.
        provider: Option<String>,
        /// Token value. Omit and use `--stdin` to read from stdin.
        token: Option<String>,
        /// Read the token from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Print manual update instructions for this build.
    #[command(visible_alias = "upgrade")]
    Update,
    /// List the supported slash commands.
    #[command(hide = true)]
    Commands,
    /// List known providers and models.
    #[command(hide = true)]
    Providers,
    /// Inspect or modify stored sessions.
    #[command(hide = true)]
    Sessions {
        #[command(subcommand)]
        command: Option<SessionCommand>,
    },
    /// List loaded declarative resources.
    #[command(hide = true)]
    Resources,
    /// Print a sample Anthropic request fixture from the current config.
    #[command(hide = true)]
    AnthropicRequestFixture,
    /// Execute a built-in tool directly for local testing.
    #[command(hide = true)]
    Tool {
        #[command(subcommand)]
        command: ToolCommand,
    },
    /// Resume a stored session in the TUI.
    #[command(hide = true)]
    Resume {
        /// The session UUID to resume.
        session_id: String,
    },
    /// Launch Puffer on a remote host over SSH.
    Remote {
        /// Hostname or SSH connection string, for example `user@buildbox`.
        target: String,
        /// Optional remote working directory before launching Puffer.
        #[arg(long = "cwd")]
        cwd: Option<String>,
        /// Disable alternate-screen mode for the remote TUI session.
        #[arg(long = "no-alt-screen", default_value_t = false)]
        no_alt_screen: bool,
        /// Optional prompt to submit on the remote side.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        prompt: Vec<String>,
    },
    /// Fork a stored session into the current working directory.
    #[command(hide = true)]
    Fork {
        /// The session UUID to fork and open.
        session_id: String,
    },
    /// Internal JSON commands used by the desktop app and remote bridges.
    #[command(hide = true)]
    DesktopApi {
        #[command(subcommand)]
        command: DesktopApiCommand,
    },
    /// Run one unattended benchmark turn and emit result artifacts.
    #[command(hide = true)]
    BenchmarkRun {
        /// Inline prompt text. Provide one of this, `--prompt-file`, or `--stdin`.
        #[arg(long = "prompt")]
        prompt: Option<String>,
        /// Read the benchmark prompt from a file.
        #[arg(long = "prompt-file")]
        prompt_file: Option<String>,
        /// Read the benchmark prompt from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
        /// Provider id to execute against.
        #[arg(long = "provider", default_value = "openai")]
        provider: String,
        /// Model id or fully-qualified selector.
        #[arg(long = "model")]
        model: String,
        /// Reasoning effort level to record in the benchmark state.
        #[arg(long = "effort", default_value = "xhigh")]
        effort: String,
        /// Enable the fast-mode toggle for unattended runs.
        #[arg(long = "fast", default_value_t = true)]
        fast: bool,
        /// Optional path for the JSON result artifact.
        #[arg(long = "result-json")]
        result_json: Option<String>,
        /// Optional path for the ATIF trajectory artifact.
        #[arg(long = "trajectory-json")]
        trajectory_json: Option<String>,
        /// Tool ids to force-deny during the run.
        #[arg(long = "deny-tool")]
        deny_tools: Vec<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum DesktopApiCommand {
    SessionGroups,
    SessionDetail {
        session_id: String,
    },
    RepoStatus {
        session_id: String,
    },
    CreatePullRequest {
        session_id: String,
        #[arg(long = "title")]
        title: Option<String>,
        #[arg(long = "body")]
        body: Option<String>,
    },
    MergePullRequest {
        session_id: String,
        #[arg(long = "pull-request-number")]
        pull_request_number: Option<u64>,
        #[arg(long = "merge-method")]
        merge_method: Option<String>,
    },
    LoginApiKey {
        provider: String,
        #[arg(long = "api-key")]
        api_key: String,
    },
    StoreCredential {
        provider: String,
        #[arg(long = "credential-json")]
        credential_json: String,
    },
    Logout {
        provider: String,
    },
    SettingsSnapshot,
}

#[derive(Debug, Subcommand)]
pub(crate) enum AuthCommand {
    /// Show authentication status.
    Status {
        /// Output as JSON.
        #[arg(long = "json", default_value_t = false, conflicts_with = "text")]
        json: bool,
        /// Output as human-readable text.
        #[arg(long = "text", default_value_t = false)]
        text: bool,
    },
    /// Sign in to a provider with OAuth.
    Login {
        /// Provider id. Defaults to the configured provider or `anthropic`.
        provider: Option<String>,
        /// Optional pasted callback URL or authorization code.
        value: Option<String>,
        /// Read the callback URL or authorization code from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Remove stored credentials for a provider.
    Logout {
        /// Provider id. Defaults to the configured provider or `anthropic`.
        provider: Option<String>,
    },
    /// Store an API key for a provider.
    #[command(hide = true)]
    SetApiKey {
        /// Provider id, for example `anthropic` or `openai`.
        provider: String,
        /// API key value. Omit and use `--stdin` to read from stdin.
        api_key: Option<String>,
        /// Read the API key from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Remove any stored credentials for a provider.
    #[command(hide = true)]
    Clear {
        /// Provider id to clear.
        provider: String,
    },
    /// Print an OAuth authorization URL for a provider when supported.
    #[command(hide = true)]
    OauthUrl {
        /// Provider id to authorize.
        provider: String,
    },
    /// Generate a full PKCE OAuth start bundle for a provider.
    #[command(hide = true)]
    OauthStart {
        /// Provider id to authorize.
        provider: String,
    },
    /// Exchange a pasted OAuth authorization code for stored credentials.
    #[command(hide = true)]
    OauthExchange {
        /// Provider id to authorize.
        provider: String,
        /// Authorization code or callback URL.
        value: Option<String>,
        /// PKCE code verifier from the `oauth-start` output.
        #[arg(long = "verifier")]
        verifier: String,
        /// Explicit OAuth state for providers that require it.
        #[arg(long = "state")]
        state: Option<String>,
        /// Read the callback URL or code from stdin.
        #[arg(long = "stdin", default_value_t = false)]
        stdin: bool,
    },
    /// Refresh stored OAuth credentials for a provider.
    #[command(hide = true)]
    OauthRefresh {
        /// Provider id to refresh.
        provider: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ToolCommand {
    /// List the registered built-in tools and their policies.
    List,
    /// Show one registered tool and its policies.
    Describe {
        /// Tool id, for example `bash`, `read_file`, or `write_file`.
        tool_id: String,
    },
    /// Run a registered tool using a JSON argument payload.
    Run {
        /// Tool id, for example `bash`, `read_file`, or `write_file`.
        tool_id: String,
        /// JSON argument payload.
        args: String,
    },
    /// Run the built-in bash tool with a shell command.
    Bash {
        /// Shell command to execute.
        command: String,
    },
    /// Run the built-in read tool against a file path.
    Read {
        /// File path to read.
        path: String,
    },
    /// Run the built-in write tool against a file path.
    Write {
        /// File path to write.
        path: String,
        /// File contents to write.
        contents: String,
    },
    /// Run the built-in replace-in-file tool against a file path.
    Replace {
        /// File path to modify.
        path: String,
        /// Text to replace.
        old: String,
        /// Replacement text.
        new: String,
        /// Replace all occurrences instead of the first match only.
        #[arg(long = "all", default_value_t = false)]
        replace_all: bool,
    },
    /// Run the built-in move-path tool.
    Move {
        /// Source path to move.
        from: String,
        /// Destination path.
        to: String,
    },
    /// Run the built-in remove-path tool.
    Remove {
        /// Path to remove.
        path: String,
        /// Remove directories recursively.
        #[arg(long = "recursive", default_value_t = false)]
        recursive: bool,
    },
    /// Run the built-in directory listing tool.
    ListDir {
        /// Optional directory path to list. Defaults to the current working directory.
        path: Option<String>,
    },
    /// Run the built-in text search tool.
    SearchText {
        /// Query text to search for.
        query: String,
        /// Optional file or directory path to search. Defaults to the current working directory.
        path: Option<String>,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum SessionCommand {
    /// List stored sessions.
    List,
    /// Set or clear the free-form note on a session.
    Note {
        /// Session UUID to update.
        session_id: String,
        /// Note text to store.
        note: Option<String>,
        /// Clear the note instead of setting it.
        #[arg(long = "clear", default_value_t = false)]
        clear: bool,
    },
    /// Add a tag to a session.
    TagAdd {
        /// Session UUID to update.
        session_id: String,
        /// Tag value to add.
        tag: String,
    },
    /// Remove a tag from a session.
    TagRemove {
        /// Session UUID to update.
        session_id: String,
        /// Tag value to remove.
        tag: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum AutoModeCommand {
    /// Print the effective Puffer auto-mode compatibility state as JSON.
    Config,
    /// Print the built-in Puffer auto-mode defaults as JSON.
    Defaults,
    /// Explain the current gap between Claude auto mode and Puffer.
    Critique,
}

#[derive(Debug, Subcommand)]
pub(crate) enum McpCommand {
    /// Add an MCP server manifest.
    Add {
        /// Stable MCP server id.
        name: String,
        /// Command or URL, depending on the selected transport.
        command_or_url: String,
        /// Additional stdio command arguments.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
        /// Transport type.
        #[arg(short = 't', long = "transport", value_enum, default_value_t = McpTransport::Stdio)]
        transport: McpTransport,
    },
    /// Add an MCP server from a JSON object.
    AddJson {
        /// Stable MCP server id.
        name: String,
        /// JSON object describing the server.
        json: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Import MCP servers from Claude Desktop.
    AddFromClaudeDesktop,
    /// Get details about one MCP server.
    Get {
        /// Stable MCP server id.
        name: String,
    },
    /// List configured MCP servers.
    List,
    /// Remove an MCP server manifest.
    Remove {
        /// Stable MCP server id.
        name: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Reset project-scoped MCP approval state.
    ResetProjectChoices,
    /// Start the Puffer MCP server bridge.
    Serve,
}

#[derive(Debug, Subcommand)]
pub(crate) enum PluginCommand {
    /// Disable an installed plugin.
    Disable {
        /// Plugin id.
        plugin: Option<String>,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Enable a disabled plugin.
    Enable {
        /// Plugin id.
        plugin: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Install a plugin manifest by id or from a local YAML path.
    Install {
        /// Plugin id or manifest path.
        plugin: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// List installed and disabled plugins.
    List,
    /// List builtin plugins that can be installed.
    Marketplace,
    /// Uninstall a plugin manifest.
    #[command(visible_alias = "remove")]
    Uninstall {
        /// Plugin id.
        plugin: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Refresh a workspace plugin from its builtin source when available.
    Update {
        /// Plugin id.
        plugin: String,
        /// Configuration scope.
        #[arg(short = 's', long = "scope", value_enum, default_value_t = ResourceScope::Local)]
        scope: ResourceScope,
    },
    /// Validate a plugin manifest.
    Validate {
        /// Path to a YAML plugin manifest.
        path: String,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum ResourceScope {
    Local,
    Project,
    User,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub(crate) enum McpTransport {
    Stdio,
    Sse,
    Http,
}

#[cfg(test)]
mod tests {
    use super::{Cli, Command};
    use clap::Parser;

    #[test]
    fn resume_flag_without_value_uses_empty_sentinel() {
        let cli = Cli::parse_from(["puffer", "--resume"]);
        assert_eq!(cli.resume.as_deref(), Some(""));
        assert!(cli.prompt.is_none());
    }

    #[test]
    fn resume_flag_with_value_keeps_positional_prompt() {
        let cli = Cli::parse_from(["puffer", "--resume", "dockyard", "follow up"]);
        assert_eq!(cli.resume.as_deref(), Some("dockyard"));
        assert_eq!(cli.prompt.as_deref(), Some("follow up"));
    }

    #[test]
    fn remote_prompt_collects_trailing_words() {
        let cli = Cli::parse_from([
            "puffer",
            "remote",
            "c@localhost",
            "--cwd",
            "/tmp/demo",
            "hello",
            "from",
            "remote",
        ]);
        let Some(Command::Remote {
            target,
            cwd,
            no_alt_screen,
            prompt,
        }) = cli.subcommand
        else {
            panic!("expected remote command");
        };
        assert_eq!(target, "c@localhost");
        assert_eq!(cwd.as_deref(), Some("/tmp/demo"));
        assert!(!no_alt_screen);
        assert_eq!(prompt, ["hello", "from", "remote"]);
    }
}
