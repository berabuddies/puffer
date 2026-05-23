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
    /// Search Telegram messages inside one peer and include context.
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
        /// Number of surrounding messages before and after each match.
        #[arg(long, default_value_t = 2)]
        context: usize,
        /// Return plain-text search results for LLM use.
        #[arg(long = "succint", alias = "succinct")]
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
