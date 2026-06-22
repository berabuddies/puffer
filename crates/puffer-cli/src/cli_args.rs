use crate::browser_args::BrowserArgs;
use crate::media_internal_tools::{ImageGenerationArgs, VideoGenerationArgs};
use crate::non_interactive::NonInteractiveArgs;
use crate::subscriber_tool_args::{EmailArgs, SlackArgs, TelegramArgs};
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
    /// Register and inspect native Puffer workflows.
    Workflow {
        #[command(subcommand)]
        command: WorkflowCommand,
    },
    /// Control the managed Chrome browser for this workspace.
    Browser(#[command(flatten)] BrowserArgs),
    /// Authenticate a connector (Telegram, Email) without a running puffer
    /// REPL. Targets GUI hosts that drive the auth flow over stdin/stdout.
    Connect {
        #[command(subcommand)]
        command: ConnectCommand,
    },
    /// Execute an internal third-party tool through a CLI surface.
    #[command(hide = true, name = "internal-tool")]
    InternalTool {
        #[command(subcommand)]
        command: InternalToolCommand,
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
    /// Internal: self-elevating Windows Chrome v20 import stages. Hidden.
    #[command(hide = true, name = "__win-chrome-import")]
    WinChromeImport {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
    /// Run Puffer in connector-only mode (Telegram, Slack, …).
    ///
    /// Reads `.puffer/connectors.toml` (workspace) or
    /// `~/.puffer/connectors.toml` (user) and starts every enabled,
    /// compiled-in platform connector. Blocks until SIGINT/SIGTERM.
    Serve {
        /// Override the connectors config path.
        #[arg(long = "config")]
        config: Option<String>,
    },
    /// Run the Puffer daemon — a WebSocket/NDJSON server that exposes the
    /// runtime (conversations, turns, permissions) to desktop / web / remote
    /// clients. One daemon per host/user config; auth is a pre-shared token.
    Daemon {
        /// Bind address. Defaults to `127.0.0.1:0` (ephemeral localhost).
        #[arg(long = "bind", default_value = "127.0.0.1:0")]
        bind: String,
        /// Path to write the auth token + bound port as NDJSON so callers
        /// can pick them up. Defaults to `<user-config>/daemon.handshake`.
        #[arg(long = "handshake-file")]
        handshake_file: Option<String>,
        /// Preset token (otherwise one is generated). Handy for tests.
        #[arg(long = "token")]
        token: Option<String>,
        /// Print a single NDJSON line to stdout once bound, then continue
        /// serving. Used by parent processes (Tauri) to pick up the port
        /// + token without racing against the file write.
        #[arg(long = "print-handshake", default_value_t = false)]
        print_handshake: bool,
        /// Disable Puffer's built-in Chrome browser RPCs for daemon runtimes.
        #[arg(long = "no-browser", default_value_t = false)]
        no_browser: bool,
        /// Additional daemon system prompt inserted after Puffer's base system prompt.
        #[arg(long = "system-prompt-1")]
        system_prompt_1: Option<String>,
        /// Disable automatic first-message title generation for daemon runtimes.
        #[arg(long = "disable-auto-title", default_value_t = false)]
        disable_auto_title: bool,
        /// Bypass permission prompts for daemon runtimes.
        #[arg(
            long = "yolo",
            alias = "dangerously-bypass-approvals-and-sandbox",
            default_value_t = false
        )]
        yolo: bool,
    },
    /// Internal: run a baked-in subscriber skill driver. Invoked by the
    /// subscriber supervisor; not intended for direct use.
    #[command(hide = true, name = "__subscriber")]
    Subscriber {
        /// Subscriber id, matches `manifest.id` (e.g. `telegram-user`).
        id: String,
    },
    /// Internal: command-backed connector protocol bridge. Invoked by the
    /// subscription manager via the connector template `command` argv (e.g.
    /// `puffer __connector lark-bot auth-ok <conn>`); not intended for direct use.
    #[command(hide = true, name = "__connector")]
    Connector {
        /// Connector id selecting the bridge implementation (e.g. `lark-bot`).
        id: String,
        /// Protocol operation (`auth-ok`, `act`, or `subscribe`) and its args.
        #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
        args: Vec<String>,
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
    /// Run one non-interactive agent turn.
    #[command(name = "non-interactive")]
    NonInteractive(NonInteractiveArgs),
}

/// Internal third-party tool command surface.
#[derive(Debug, Subcommand)]
pub(crate) enum InternalToolCommand {
    /// Print shell aliases for internal tools.
    Aliases,
    /// Control the managed Chrome browser for this workspace.
    Browser(#[command(flatten)] BrowserArgs),
    /// Configure the email subscriber through the parent runtime.
    Email(#[command(flatten)] EmailArgs),
    /// Generate images through the parent media runtime.
    #[command(name = "image-generation", alias = "imagegen")]
    ImageGeneration(#[command(flatten)] ImageGenerationArgs),
    /// Log in to Slack or look up Slack conversations through the parent runtime.
    Slack(#[command(flatten)] SlackArgs),
    /// Log in to Telegram or look up Telegram peers through the parent runtime.
    Telegram(#[command(flatten)] TelegramArgs),
    /// Generate a text-to-video clip through the parent media runtime.
    #[command(name = "video-generation", alias = "videogen")]
    VideoGeneration(#[command(flatten)] VideoGenerationArgs),
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
    WorkflowList,
    WorkflowRunsList {
        workflow_slug: String,
    },
    WorkflowRunsShow {
        idx: u64,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowCommand {
    /// Register a workflow envelope or raw AgentFlow pipeline JSON file.
    Register {
        /// JSON file path.
        json_path: String,
        /// Optional workflow slug override.
        #[arg(long = "slug")]
        slug: Option<String>,
        /// Cron trigger for raw AgentFlow pipeline imports.
        #[arg(long = "cron", conflicts_with = "subscription_topic")]
        cron: Option<String>,
        /// Connection slug for raw AgentFlow pipeline imports.
        #[arg(
            long = "connection-slug",
            alias = "subscription-topic",
            conflicts_with = "cron"
        )]
        connection_slug: Option<String>,
        /// Optional connection regex filter.
        #[arg(long = "connection-pattern", alias = "subscription-pattern")]
        connection_pattern: Option<String>,
        /// Optional subscription classifier prompt.
        #[arg(long = "classify-prompt")]
        classify_prompt: Option<String>,
    },
    /// List registered workflows.
    Ls {
        /// Output JSON.
        #[arg(long = "json", default_value_t = false)]
        json: bool,
    },
    /// Inspect workflow run records.
    Runs {
        #[command(subcommand)]
        command: WorkflowRunsCommand,
    },
    /// Run one workflow once. Use --dry-run for deterministic local execution.
    Run {
        /// Workflow slug.
        workflow_slug: String,
        /// Trigger JSON payload used for prompt interpolation.
        #[arg(long = "trigger-json")]
        trigger_json: Option<String>,
        /// Use a deterministic local executor instead of calling an LLM provider.
        #[arg(long = "dry-run", default_value_t = false)]
        dry_run: bool,
        /// Output JSON.
        #[arg(long = "json", default_value_t = false)]
        json: bool,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum WorkflowRunsCommand {
    /// List runs for a workflow slug.
    Ls {
        /// Workflow slug.
        workflow_slug: String,
        /// Output JSON.
        #[arg(long = "json", default_value_t = false)]
        json: bool,
    },
    /// Show one run by global index.
    Show {
        /// Global run index.
        idx: u64,
        /// Output JSON.
        #[arg(long = "json", default_value_t = false)]
        json: bool,
    },
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
    /// Drive the OAuth interactive login for an HTTP MCP server.
    ///
    /// Runs RFC 8414 metadata discovery, RFC 7591 dynamic client
    /// registration, the authorization-code-with-PKCE flow against the
    /// server's authorization endpoint, captures the callback on a
    /// localhost loopback port, and persists the resulting tokens under
    /// `<config>/puffer/mcp-tokens/`. Tokens auto-refresh on expiry from
    /// the runner the next time the server is contacted.
    Login {
        /// Stable MCP server id (must match a configured server with
        /// `transport: http` and `oauth: true`).
        name: String,
        /// Skip the `webbrowser::open` attempt and just print the URL.
        /// Useful for headless environments (SSH, container, CI). The
        /// callback receiver still binds locally — pair this with a
        /// reverse-tunnel or a manual paste-back of the redirect URL.
        #[arg(long = "no-browser")]
        no_browser: bool,
    },
    /// Print whether OAuth tokens exist on disk for an MCP server, and
    /// where they live.
    LoginStatus {
        /// Stable MCP server id.
        name: String,
    },
    /// Forget any persisted OAuth tokens for an MCP server. The next
    /// connect will require a fresh `puffer mcp login`.
    Logout {
        /// Stable MCP server id.
        name: String,
    },
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

#[derive(Debug, Subcommand)]
pub(crate) enum ConnectCommand {
    /// Drive a Telegram personal-account login via a JSON-stdio REPL.
    ///
    /// Stdin accepts newline-delimited JSON objects, one per line.
    /// Recognised actions: `login_start { phone, [api_id], [api_hash] }`,
    /// `submit_code { code }`, `submit_password { password }`, `exit`.
    /// Stdout emits the subscriber's control events (`login_awaiting_code`,
    /// `login_awaiting_password`, `login_complete`, `login_error`).
    Telegram {
        /// Telegram connection slug. Distinct slugs scope independent
        /// account sessions. Defaults to `telegram-user`.
        #[arg(long = "connection", default_value = "telegram-user")]
        connection_slug: String,
    },
    /// Persist Email (IMAP/SMTP) credentials for the email subscriber.
    Email {
        #[command(subcommand)]
        command: ConnectEmailCommand,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum ConnectEmailCommand {
    /// Writes IMAP/SMTP credentials to the email subscriber's config file.
    Configure {
        /// IMAP server host, e.g. `imap.gmail.com`.
        #[arg(long = "imap-host")]
        imap_host: String,
        /// IMAP server port. `0` lets the subscriber default it (993).
        #[arg(long = "imap-port", default_value_t = 0)]
        imap_port: u16,
        /// SMTP server host, e.g. `smtp.gmail.com`.
        #[arg(long = "smtp-host")]
        smtp_host: String,
        /// SMTP server port. `0` lets the subscriber default it (587).
        #[arg(long = "smtp-port", default_value_t = 0)]
        smtp_port: u16,
        /// IMAP/SMTP login (usually the full email address).
        #[arg(long)]
        username: String,
        /// IMAP/SMTP password or app-specific password.
        #[arg(
            long,
            required_unless_present = "password_stdin",
            conflicts_with = "password_stdin"
        )]
        password: Option<String>,
        /// Read the password from stdin (newline-terminated).
        #[arg(long = "password-stdin", default_value_t = false)]
        password_stdin: bool,
        /// From address used on outbound mail.
        #[arg(long = "from-address")]
        from_address: String,
        /// Optional allow-listed sender; repeat for multiple entries.
        #[arg(long = "allowed-sender")]
        allowed_senders: Vec<String>,
    },
}
