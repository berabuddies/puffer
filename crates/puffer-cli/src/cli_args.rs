use clap::{Args, Parser, Subcommand, ValueEnum};
use std::path::PathBuf;

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
    /// clients. One daemon per workspace; auth is a pre-shared token.
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
        /// Disable Puffer's built-in Chrome browser RPCs for managed-agent runtimes.
        #[arg(long = "no-browser", default_value_t = false)]
        no_browser: bool,
        /// Additional managed-agent system prompt inserted after Puffer's base system prompt.
        #[arg(long = "system-prompt-1")]
        system_prompt_1: Option<String>,
    },
    /// Internal: run a baked-in subscriber skill driver. Invoked by the
    /// subscriber supervisor; not intended for direct use.
    #[command(hide = true, name = "__subscriber")]
    Subscriber {
        /// Subscriber id, matches `manifest.id` (e.g. `telegram-user`).
        id: String,
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

/// Top-level CLI arguments for `puffer browser`.
#[derive(Debug, Args)]
pub(crate) struct BrowserArgs {
    /// Output machine-readable JSON instead of text.
    #[arg(long = "json", global = true, default_value_t = false)]
    pub(crate) json: bool,
    /// Explicit root browser session id. Use this to share tabs with another session.
    #[arg(long = "session-id", global = true)]
    pub(crate) session_id: Option<String>,
    #[command(subcommand)]
    pub(crate) command: BrowserCommand,
}

#[derive(Clone, Debug, Args)]
pub(crate) struct BrowserTargetArgs {
    /// Stable browser tab id such as `t1`.
    #[arg(long = "tab-id")]
    pub(crate) tab_id: Option<String>,
    /// Optional viewport width.
    #[arg(long = "width")]
    pub(crate) width: Option<u32>,
    /// Optional viewport height.
    #[arg(long = "height")]
    pub(crate) height: Option<u32>,
}

#[derive(Debug, Subcommand)]
pub(crate) enum BrowserTabCommand {
    /// List managed browser tabs for the selected root session.
    List,
    /// Open a fresh managed browser tab.
    New {
        /// Optional URL to open.
        url: Option<String>,
        /// Stable browser tab id such as `t3`.
        #[arg(long = "tab-id")]
        tab_id: Option<String>,
        /// Optional stable label for the tab.
        #[arg(long = "label")]
        label: Option<String>,
        /// Optional viewport width.
        #[arg(long = "width")]
        width: Option<u32>,
        /// Optional viewport height.
        #[arg(long = "height")]
        height: Option<u32>,
    },
    /// Close one managed browser tab, defaulting to the active tab.
    Close {
        /// Stable browser tab id such as `t1`.
        tab_id: Option<String>,
    },
    /// Focus an existing managed browser tab by id.
    #[command(visible_alias = "select")]
    Focus {
        /// Stable browser tab id such as `t4`.
        tab_id: String,
    },
}

#[derive(Debug, Subcommand)]
pub(crate) enum BrowserKeyboardCommand {
    /// Type text into the active or selected tab.
    Type {
        /// Text to insert.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Insert raw text without synthesizing per-key events.
    InsertText {
        /// Text to insert.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
}

/// Browser CLI subcommands backed by the managed daemon browser surface.
#[derive(Debug, Subcommand)]
pub(crate) enum BrowserCommand {
    /// List managed browser tabs for the selected root session.
    List,
    /// Open or reuse a managed browser tab, optionally at a URL.
    Open {
        /// Optional URL to open.
        url: Option<String>,
        /// Stable browser tab id such as `t1`.
        #[arg(long = "tab-id")]
        tab_id: Option<String>,
        /// Optional stable label for the tab.
        #[arg(long = "label")]
        label: Option<String>,
        /// Optional viewport width.
        #[arg(long = "width")]
        width: Option<u32>,
        /// Optional viewport height.
        #[arg(long = "height")]
        height: Option<u32>,
    },
    /// Navigate the active or selected tab to a URL.
    #[command(visible_alias = "goto")]
    Navigate {
        /// Destination URL.
        url: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Move back in browser history.
    Back {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Move forward in browser history.
    Forward {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Reload the active or selected tab.
    Reload {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Close one managed browser tab, defaulting to the active tab.
    Close {
        #[arg(long = "group", default_value_t = false)]
        group: bool,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Close every managed tab in the selected browser session.
    #[command(visible_alias = "exit")]
    Quit,
    /// Manage tabs within the selected browser session.
    Tab {
        #[command(subcommand)]
        command: BrowserTabCommand,
    },
    /// Capture a DOM snapshot with fresh element refs.
    Snapshot {
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Capture one still screenshot of the active or selected tab.
    Screenshot {
        /// Optional output path. When omitted, Puffer writes under `.puffer/screenshots/`.
        #[arg(value_name = "PATH")]
        path: Option<PathBuf>,
        /// Overlay fresh `@eN` ref labels onto the captured screenshot.
        #[arg(long = "annotate", default_value_t = false)]
        annotate: bool,
        /// Directory for auto-generated screenshot files when no `PATH` is provided.
        #[arg(long = "screenshot-dir", value_name = "DIR")]
        screenshot_dir: Option<PathBuf>,
        /// Image format for the captured screenshot. Defaults to `png`.
        #[arg(long = "screenshot-format", value_parser = ["png", "jpeg"])]
        screenshot_format: Option<String>,
        /// JPEG quality from 0 to 100.
        #[arg(long = "screenshot-quality", value_parser = clap::value_parser!(u8).range(0..=100))]
        screenshot_quality: Option<u8>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Click an element ref from the latest snapshot.
    Click {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Double-click an element ref from the latest snapshot.
    Dblclick {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Hover one element ref from the latest snapshot.
    Hover {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Focus one element ref from the latest snapshot.
    #[command(visible_alias = "focus-ref")]
    Focus {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Replace text in an editable element ref from the latest snapshot.
    Fill {
        /// Element ref such as `@e5`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// Replacement text.
        text: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Select one option in a native `<select>` ref from the latest snapshot.
    ///
    /// The current ref model matches one option by exact option value or label text.
    Select {
        /// Element ref such as `@e6`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// Exact option value or label text.
        value: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Upload one or more files into a native `<input type="file">` ref.
    Upload {
        /// Element ref such as `@e9`.
        #[arg(value_name = "REF")]
        ref_id: String,
        /// One or more file paths to attach.
        #[arg(value_name = "FILE", required = true, num_args = 1..)]
        files: Vec<PathBuf>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Check one checkbox-like ref from the latest snapshot.
    Check {
        /// Element ref such as `@e7`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Uncheck one checkbox-like ref from the latest snapshot.
    ///
    /// The current ref model does not support unchecking radio buttons directly.
    Uncheck {
        /// Element ref such as `@e7`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Type text into the page or into one target ref.
    Type {
        /// Text to insert.
        text: String,
        /// Optional element ref to focus before typing.
        #[arg(long = "ref", value_name = "REF")]
        ref_id: Option<String>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Press one key in the active or selected tab.
    #[command(visible_alias = "key")]
    Press {
        /// Key name such as `Enter`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Hold one key down in the active or selected tab.
    Keydown {
        /// Key name such as `Shift`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Release one key in the active or selected tab.
    Keyup {
        /// Key name such as `Shift`.
        key: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Send page-level keyboard actions.
    Keyboard {
        #[command(subcommand)]
        command: BrowserKeyboardCommand,
    },
    /// Scroll the page in one direction.
    Scroll {
        /// Direction: up, down, left, or right.
        direction: String,
        /// Optional pixel distance. Defaults to 600.
        px: Option<u32>,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Scroll one ref into view from the latest snapshot.
    #[command(visible_alias = "scrollinto")]
    ScrollIntoView {
        /// Element ref such as `@e3`.
        #[arg(value_name = "REF")]
        ref_id: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
    },
    /// Evaluate one JavaScript expression in the active or selected tab.
    #[command(visible_alias = "evaluate")]
    Eval {
        /// JavaScript expression.
        script: String,
        #[command(flatten)]
        target: BrowserTargetArgs,
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
        /// Subscription source topic for raw AgentFlow pipeline imports.
        #[arg(long = "subscription-topic", conflicts_with = "cron")]
        subscription_topic: Option<String>,
        /// Optional subscription regex prefilter.
        #[arg(long = "subscription-pattern")]
        subscription_pattern: Option<String>,
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
