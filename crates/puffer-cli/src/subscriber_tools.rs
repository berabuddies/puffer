//! Internal CLI adapters for subscriber-backed Email and Telegram tools.

use crate::subscriber_tool_args::{EmailArgs, EmailCommand, TelegramArgs, TelegramCommand};
use anyhow::{bail, Context, Result};
use puffer_tools::internal_permissions::{
    require_internal_tool_execution_from_env, InternalToolExecutionRequest,
};
use serde_json::{json, Value};
use std::io::Read as _;

/// Runs one `puffer internal-tool email` command.
pub(crate) fn run_email(args: EmailArgs) -> Result<()> {
    let input = match args.command {
        EmailCommand::Configure {
            imap_host,
            imap_port,
            smtp_host,
            smtp_port,
            username,
            password,
            password_stdin,
            from_address,
            allowed_senders,
        } => {
            let password = read_secret_arg(password, password_stdin, "email password")?;
            json!({
                "action": "configure",
                "imap_host": imap_host,
                "imap_port": imap_port,
                "smtp_host": smtp_host,
                "smtp_port": smtp_port,
                "username": username,
                "password": password,
                "from_address": from_address,
                "allowed_senders": allowed_senders,
            })
        }
    };
    execute_parent_internal_tool("email", input)
}

/// Runs one `puffer internal-tool telegram` command.
pub(crate) fn run_telegram(args: TelegramArgs) -> Result<()> {
    let connection_slug = args.connection_slug;
    let mut input = match args.command {
        TelegramCommand::ImportDesktop {
            path,
            account_index,
            passcode,
            passcode_stdin,
            key_file,
        } => {
            let passcode =
                optional_secret_arg(passcode, passcode_stdin, "Telegram local passcode")?;
            json!({
                "action": "import_desktop",
                "path": path,
                "account_index": account_index,
                "passcode": passcode,
                "key_file": key_file,
            })
        }
        TelegramCommand::ListPeers {
            peer_kind,
            query,
            limit,
        } => json!({
            "action": "list_peers",
            "peer_kind": peer_kind.map(|kind| kind.as_str()),
            "query": query,
            "limit": limit,
        }),
        TelegramCommand::SearchPeers {
            query,
            peer_kind,
            limit,
        } => json!({
            "action": "search_peers",
            "peer_kind": peer_kind.map(|kind| kind.as_str()),
            "query": query,
            "limit": limit,
        }),
        TelegramCommand::SearchMessages {
            query,
            peer,
            limit,
            context,
            succinct,
        } => json!({
            "action": "search_messages",
            "peer": peer,
            "query": query,
            "limit": limit,
            "context": context,
            "succinct": succinct,
        }),
        TelegramCommand::LoginQr { api_id, api_hash } => json!({
            "action": "login_qr",
            "api_id": api_id,
            "api_hash": api_hash,
        }),
        TelegramCommand::LoginQrWait { timeout_seconds } => json!({
            "action": "login_qr_wait",
            "timeout_seconds": timeout_seconds,
        }),
        TelegramCommand::LoginStart {
            phone,
            api_id,
            api_hash,
        } => json!({
            "action": "login_start",
            "phone": phone,
            "api_id": api_id,
            "api_hash": api_hash,
        }),
        TelegramCommand::LoginSubmitCode { code } => json!({
            "action": "login_submit_code",
            "code": code,
        }),
        TelegramCommand::LoginSubmitPassword {
            password,
            password_stdin,
        } => {
            let password = read_secret_arg(password, password_stdin, "Telegram 2FA password")?;
            json!({
                "action": "login_submit_password",
                "password": password,
            })
        }
    };
    if let Some(object) = input.as_object_mut() {
        object.insert(
            "connection_slug".to_string(),
            Value::String(connection_slug),
        );
    }
    execute_parent_internal_tool("telegram", input)
}

fn execute_parent_internal_tool(tool_id: &str, input: Value) -> Result<()> {
    let response = require_internal_tool_execution_from_env(InternalToolExecutionRequest {
        tool_id: tool_id.to_string(),
        input,
    })?;
    if !response.success {
        bail!(
            "{tool_id} internal tool failed: {}",
            response
                .reason
                .unwrap_or_else(|| "unknown error".to_string())
        );
    }
    if let Some(output) = response.output {
        println!("{output}");
    }
    Ok(())
}

fn read_secret_arg(value: Option<String>, from_stdin: bool, label: &str) -> Result<String> {
    if !from_stdin {
        return value.with_context(|| format!("{label} is required"));
    }
    read_secret_from_stdin(label)
}

fn optional_secret_arg(
    value: Option<String>,
    from_stdin: bool,
    label: &str,
) -> Result<Option<String>> {
    if !from_stdin {
        return Ok(value);
    }
    read_secret_from_stdin(label).map(Some)
}

fn read_secret_from_stdin(label: &str) -> Result<String> {
    let mut secret = String::new();
    std::io::stdin()
        .read_to_string(&mut secret)
        .with_context(|| format!("read {label} from stdin"))?;
    Ok(secret.trim_end_matches(['\r', '\n']).to_string())
}
