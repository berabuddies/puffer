use clap::{Args, Subcommand};

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
    #[command(subcommand)]
    pub(crate) command: TelegramCommand,
}

#[derive(Debug, Subcommand)]
pub(crate) enum TelegramCommand {
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
