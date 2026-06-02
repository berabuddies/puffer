use clap::{Args, Subcommand, ValueEnum};

#[derive(Debug, Args)]
pub(crate) struct EmailArgs {
    #[command(subcommand)]
    pub(crate) command: EmailCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum EmailCommand {
    /// Configure the email subscriber credentials.
    Configure {
        /// IMAP server host, for example imap.gmail.com.
        #[arg(long = "imap-host")]
        imap_host: String,
        /// IMAP server port. Use 0 to let the subscriber default it.
        #[arg(long = "imap-port", default_value_t = 0)]
        imap_port: u16,
        /// SMTP server host, for example smtp.gmail.com.
        #[arg(long = "smtp-host")]
        smtp_host: String,
        /// SMTP server port. Use 0 to let the subscriber default it.
        #[arg(long = "smtp-port", default_value_t = 0)]
        smtp_port: u16,
        /// Login username, usually the full email address.
        #[arg(long)]
        username: String,
        /// Email password or app-specific password.
        #[arg(
            long,
            required_unless_present = "password_stdin",
            conflicts_with = "password_stdin"
        )]
        password: Option<String>,
        /// Read the email password from stdin.
        #[arg(long = "password-stdin")]
        password_stdin: bool,
        /// From address to use for outbound email.
        #[arg(long = "from-address")]
        from_address: String,
        /// Optional allowed sender address or domain suffix. Repeat as needed.
        #[arg(long = "allowed-sender")]
        allowed_senders: Vec<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct LarkArgs {
    /// Lark connection slug. Defaults to lark-app for configure-app and app env imports, lark-login otherwise.
    #[arg(
        long = "connection",
        aliases = ["connection-slug", "account", "account-slug"]
    )]
    pub(crate) connection_slug: Option<String>,
    #[command(subcommand)]
    pub(crate) command: LarkCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum LarkCommand {
    /// Configure a Lark app with app id and app secret.
    #[command(name = "configure-app")]
    ConfigureApp {
        /// Lark app id, usually cli_...
        #[arg(long = "app-id")]
        app_id: String,
        /// Lark app secret.
        #[arg(
            long = "app-secret",
            required_unless_present = "app_secret_stdin",
            conflicts_with = "app_secret_stdin"
        )]
        app_secret: Option<String>,
        /// Read the Lark app secret from stdin.
        #[arg(long = "app-secret-stdin")]
        app_secret_stdin: bool,
        /// Lark endpoint brand.
        #[arg(long, value_enum, default_value_t = LarkBrandArg::Lark)]
        brand: LarkBrandArg,
    },
    /// Import Lark credentials from LARK_* environment variables.
    #[command(name = "import-env")]
    ImportEnv {
        /// Override LARK_BRAND for this import.
        #[arg(long, value_enum)]
        brand: Option<LarkBrandArg>,
    },
    /// Configure a Lark user access token login.
    #[command(name = "login-token")]
    LoginToken {
        /// Lark user access token.
        #[arg(
            long = "user-access-token",
            aliases = ["token"],
            required_unless_present = "user_access_token_stdin",
            conflicts_with = "user_access_token_stdin"
        )]
        user_access_token: Option<String>,
        /// Read the Lark user access token from stdin.
        #[arg(long = "user-access-token-stdin", alias = "token-stdin")]
        user_access_token_stdin: bool,
        /// Optional app id associated with the user token.
        #[arg(long = "app-id")]
        app_id: Option<String>,
        /// Lark endpoint brand.
        #[arg(long, value_enum, default_value_t = LarkBrandArg::Lark)]
        brand: LarkBrandArg,
    },
    /// List Lark chats visible to the connection.
    #[command(name = "list-chats")]
    ListChats {
        /// Maximum number of chats to return.
        #[arg(long = "page-size", default_value_t = 100)]
        page_size: usize,
        /// Optional Lark pagination token.
        #[arg(long = "page-token")]
        page_token: Option<String>,
    },
    /// Search Lark group chats by name.
    #[command(name = "search-chats")]
    SearchChats {
        /// Chat search query.
        query: String,
        /// Optional comma-separated chat types: private, external, public_joined, public_not_joined.
        #[arg(long = "search-types")]
        search_types: Option<String>,
        /// Maximum number of chats to return.
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: usize,
        /// Optional Lark pagination token.
        #[arg(long = "page-token")]
        page_token: Option<String>,
    },
    /// Search Lark users by name or open_id filters.
    #[command(name = "search-users")]
    SearchUsers {
        /// Optional user search query.
        #[arg(long)]
        query: Option<String>,
        /// Optional comma-separated open_ids to look up or restrict against.
        #[arg(long = "user-ids")]
        user_ids: Option<String>,
        /// Restrict to users the token has chatted with.
        #[arg(long = "has-chatted")]
        has_chatted: bool,
        /// Exclude cross-tenant external users.
        #[arg(long = "exclude-external-users")]
        exclude_external_users: bool,
        /// Maximum number of users to return.
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: usize,
    },
    /// Read Lark chat messages.
    #[command(name = "read-messages")]
    ReadMessages {
        /// Lark chat id, usually oc_...
        #[arg(
            long = "chat-id",
            required_unless_present = "thread_id",
            conflicts_with = "thread_id"
        )]
        chat_id: Option<String>,
        /// Lark thread id, usually omt_..., for reading thread replies.
        #[arg(
            long = "thread-id",
            required_unless_present = "chat_id",
            conflicts_with = "chat_id"
        )]
        thread_id: Option<String>,
        /// Maximum number of messages to return.
        #[arg(long = "page-size", default_value_t = 50)]
        page_size: usize,
        /// Optional Lark pagination token.
        #[arg(long = "page-token")]
        page_token: Option<String>,
        /// Sort order: asc or desc.
        #[arg(long)]
        sort: Option<String>,
        /// Optional Lark start_time value.
        #[arg(long = "start-time")]
        start_time: Option<String>,
        /// Optional Lark end_time value.
        #[arg(long = "end-time")]
        end_time: Option<String>,
    },
    /// Search Lark messages.
    #[command(name = "search-messages")]
    SearchMessages {
        /// Lark message search query.
        query: String,
        /// Optional comma-separated chat ids to restrict search.
        #[arg(long = "chat-ids")]
        chat_ids: Option<String>,
        /// Optional comma-separated sender open_ids to restrict search.
        #[arg(long = "sender-ids")]
        sender_ids: Option<String>,
        /// Optional chat type filter: group or p2p.
        #[arg(long = "chat-type")]
        chat_type: Option<String>,
        /// Maximum number of messages to return.
        #[arg(long = "page-size", default_value_t = 20)]
        page_size: usize,
        /// Optional Lark pagination token.
        #[arg(long = "page-token")]
        page_token: Option<String>,
    },
    /// Batch fetch Lark message details by message ids.
    #[command(name = "mget-messages")]
    MgetMessages {
        /// Comma-separated Lark message ids, usually om_...
        #[arg(long = "message-ids")]
        message_ids: String,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum LarkBrandArg {
    Lark,
    Feishu,
}

impl LarkBrandArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Lark => "lark",
            Self::Feishu => "feishu",
        }
    }
}

#[derive(Debug, Args)]
pub(crate) struct SlackArgs {
    /// Slack connection slug. Defaults to slack-app for configure-app and slack-login otherwise.
    #[arg(
        long = "connection",
        aliases = ["connection-slug", "account", "account-slug"]
    )]
    pub(crate) connection_slug: Option<String>,
    #[command(subcommand)]
    pub(crate) command: SlackCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum SlackCommand {
    /// Configure a Slack app with bot and app-level tokens.
    #[command(name = "configure-app")]
    ConfigureApp {
        /// Slack bot token, usually xoxb-...
        #[arg(
            long = "bot-token",
            required_unless_present = "bot_token_stdin",
            conflicts_with = "bot_token_stdin"
        )]
        bot_token: Option<String>,
        /// Read the Slack bot token from stdin.
        #[arg(long = "bot-token-stdin")]
        bot_token_stdin: bool,
        /// Slack app-level token, usually xapp-...
        #[arg(
            long = "app-token",
            required_unless_present = "app_token_stdin",
            conflicts_with = "app_token_stdin"
        )]
        app_token: Option<String>,
        /// Read the Slack app-level token from stdin.
        #[arg(long = "app-token-stdin")]
        app_token_stdin: bool,
        /// Optional workspace name to store before auth.test fills it.
        #[arg(long = "workspace-name")]
        workspace_name: Option<String>,
    },
    /// Configure a Slack OAuth token login.
    #[command(name = "login-token")]
    LoginToken {
        /// Slack OAuth token, usually xoxb-... or xoxp-...
        #[arg(
            long,
            required_unless_present = "token_stdin",
            conflicts_with = "token_stdin"
        )]
        token: Option<String>,
        /// Read the Slack OAuth token from stdin.
        #[arg(long = "token-stdin")]
        token_stdin: bool,
        /// Optional token kind label.
        #[arg(long = "token-type", default_value = "oauth")]
        token_type: String,
        /// Optional workspace name to store before auth.test fills it.
        #[arg(long = "workspace-name")]
        workspace_name: Option<String>,
    },
    /// Configure Slack browser tokens from a local browser/app session.
    #[command(name = "login-browser")]
    LoginBrowser {
        /// Slack workspace URL, for example https://example.slack.com.
        #[arg(long = "workspace-url")]
        workspace_url: String,
        /// Slack browser d cookie, usually xoxd-...
        #[arg(
            long = "xoxd",
            required_unless_present = "xoxd_stdin",
            conflicts_with = "xoxd_stdin"
        )]
        xoxd_token: Option<String>,
        /// Read the Slack xoxd cookie from stdin.
        #[arg(long = "xoxd-stdin")]
        xoxd_stdin: bool,
        /// Slack browser API token, usually xoxc-...
        #[arg(
            long = "xoxc",
            required_unless_present = "xoxc_stdin",
            conflicts_with = "xoxc_stdin"
        )]
        xoxc_token: Option<String>,
        /// Read the Slack xoxc token from stdin.
        #[arg(long = "xoxc-stdin")]
        xoxc_stdin: bool,
        /// Optional workspace name to store before auth.test fills it.
        #[arg(long = "workspace-name")]
        workspace_name: Option<String>,
    },
    /// Import Slack browser auth from a local Slack app profile.
    #[command(name = "import-local")]
    ImportLocal {
        /// Optional Slack app data directory to scan.
        #[arg(long)]
        path: Option<String>,
        /// Optional workspace URL to disambiguate local multi-workspace data.
        #[arg(long = "workspace-url")]
        workspace_url: Option<String>,
    },
    /// List Slack conversations.
    #[command(name = "list-conversations")]
    ListConversations {
        /// Slack conversation types to include.
        #[arg(long, default_value = "public_channel,private_channel,mpim,im")]
        types: String,
        /// Maximum number of conversations to return.
        #[arg(long, default_value_t = 100)]
        limit: usize,
        /// Optional Slack pagination cursor.
        #[arg(long)]
        cursor: Option<String>,
        /// Include archived conversations.
        #[arg(long = "include-archived")]
        include_archived: bool,
    },
    /// Search Slack conversations by name, id, or user id.
    #[command(name = "search-conversations")]
    SearchConversations {
        /// Case-insensitive conversation search query.
        query: String,
        /// Slack conversation types to include.
        #[arg(long, default_value = "public_channel,private_channel,mpim,im")]
        types: String,
        /// Maximum number of matching conversations to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Search Slack users by name, handle, id, or email.
    #[command(name = "search-users")]
    SearchUsers {
        /// Case-insensitive user search query.
        query: String,
        /// Maximum number of matching users to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// Read Slack channel messages or thread replies.
    #[command(name = "read-messages")]
    ReadMessages {
        /// Slack channel/conversation id.
        #[arg(long)]
        channel: String,
        /// Optional Slack thread timestamp for replies.
        #[arg(long = "thread-ts")]
        thread_ts: Option<String>,
        /// Maximum number of messages to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
        /// Optional oldest Slack timestamp.
        #[arg(long)]
        oldest: Option<String>,
        /// Optional latest Slack timestamp.
        #[arg(long)]
        latest: Option<String>,
    },
    /// Search Slack messages with Slack search syntax.
    #[command(name = "search-messages")]
    SearchMessages {
        /// Slack search query.
        query: String,
        /// Maximum number of matches to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Optional Slack search page.
        #[arg(long)]
        page: Option<usize>,
        /// Optional Slack sort, such as score or timestamp.
        #[arg(long)]
        sort: Option<String>,
        /// Optional Slack sort direction, such as asc or desc.
        #[arg(long = "sort-dir")]
        sort_dir: Option<String>,
    },
}

#[derive(Debug, Args)]
pub(crate) struct TelegramArgs {
    /// Telegram account connection slug. Use distinct slugs for multiple local accounts.
    #[arg(
        long = "connection",
        aliases = ["connection-slug", "account", "account-slug"],
        default_value = "telegram-user"
    )]
    pub(crate) connection_slug: String,
    #[command(subcommand)]
    pub(crate) command: TelegramCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TelegramCommand {
    /// Import authentication from a local Telegram Desktop tdata folder.
    #[command(name = "import-desktop")]
    ImportDesktop {
        /// Path to Telegram Desktop's tdata directory. Defaults to the
        /// platform Telegram Desktop location.
        #[arg(long)]
        path: Option<String>,
        /// Zero-based Telegram Desktop account slot. Defaults to the main
        /// account.
        #[arg(long = "account-index")]
        account_index: Option<usize>,
        /// Telegram Desktop local passcode.
        #[arg(long, conflicts_with = "passcode_stdin")]
        passcode: Option<String>,
        /// Read the Telegram Desktop local passcode from stdin.
        #[arg(long = "passcode-stdin")]
        passcode_stdin: bool,
        /// Telegram Desktop tdata key file name. Defaults to `data`.
        #[arg(long = "key-file")]
        key_file: Option<String>,
    },
    /// List Telegram users, groups, and channels visible in dialog history.
    #[command(name = "list-peers")]
    ListPeers {
        /// Optional peer type filter.
        #[arg(long = "kind")]
        peer_kind: Option<TelegramPeerKindArg>,
        /// Optional case-insensitive search query.
        #[arg(long)]
        query: Option<String>,
        /// Maximum number of peers to return.
        #[arg(long, default_value_t = 50)]
        limit: usize,
    },
    /// Search Telegram users, groups, and channels by title, username, or id.
    #[command(name = "search-peers")]
    SearchPeers {
        /// Case-insensitive search query.
        query: String,
        /// Optional peer type filter.
        #[arg(long = "kind")]
        peer_kind: Option<TelegramPeerKindArg>,
        /// Maximum number of peers to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
    },
    /// List recent Telegram messages inside one peer without a search term.
    #[command(name = "list-messages", alias = "messages")]
    ListMessages {
        /// Telegram peer id from search-peers, or a public @username.
        #[arg(long)]
        peer: String,
        /// Maximum number of messages to return.
        #[arg(long, default_value_t = 20)]
        limit: usize,
        /// Exclusive Telegram message id cursor for older messages.
        #[arg(long = "before-id")]
        before_id: Option<i32>,
        /// Return plain-text messages for LLM use.
        #[arg(long = "succinct")]
        succinct: bool,
    },
    /// Search Telegram messages inside one peer and include previous context.
    #[command(name = "search-messages")]
    SearchMessages {
        /// Text query to search for.
        query: String,
        /// Telegram peer id from search-peers, or a public @username.
        #[arg(long)]
        peer: String,
        /// Maximum number of message matches to return.
        #[arg(long, default_value_t = 10)]
        limit: usize,
        /// Number of previous messages to include before each match.
        #[arg(long, default_value_t = 0)]
        context: usize,
        /// Return plain-text search results for LLM use.
        #[arg(long = "succinct")]
        succinct: bool,
    },
    /// Start Telegram QR login for approval from an already logged-in app.
    LoginQr {
        /// Optional Telegram application API id.
        #[arg(long = "api-id")]
        api_id: Option<i32>,
        /// Optional Telegram application API hash.
        #[arg(long = "api-hash")]
        api_hash: Option<String>,
    },
    /// Wait for approval of the active Telegram QR login.
    LoginQrWait {
        /// Optional wait timeout in seconds.
        #[arg(long = "timeout-seconds")]
        timeout_seconds: Option<u64>,
    },
    /// Start Telegram personal-account login.
    LoginStart {
        /// E.164 phone number, for example +15551234567.
        phone: String,
        /// Optional Telegram application API id.
        #[arg(long = "api-id")]
        api_id: Option<i32>,
        /// Optional Telegram application API hash.
        #[arg(long = "api-hash")]
        api_hash: Option<String>,
    },
    /// Submit the Telegram login code.
    LoginSubmitCode {
        /// Numeric login code delivered by Telegram.
        code: String,
    },
    /// Submit the Telegram 2FA cloud password.
    LoginSubmitPassword {
        /// Telegram 2FA cloud password.
        #[arg(
            long,
            required_unless_present = "password_stdin",
            conflicts_with = "password_stdin"
        )]
        password: Option<String>,
        /// Read the 2FA cloud password from stdin.
        #[arg(long = "password-stdin")]
        password_stdin: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
pub(crate) enum TelegramPeerKindArg {
    User,
    Group,
    Channel,
}

impl TelegramPeerKindArg {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::User => "user",
            Self::Group => "group",
            Self::Channel => "channel",
        }
    }
}
