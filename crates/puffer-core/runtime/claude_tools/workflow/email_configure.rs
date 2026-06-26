//! Email subscriber workflow actions.
//!
//! The consolidated internal `Email` tool installs IMAP/SMTP credentials into
//! the email subscriber, persists them to its state directory, and starts the
//! polling loop.

use anyhow::{anyhow, Context, Result};
use puffer_subscriber_runtime::{Manifest, SubscriberCommand};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

use super::subscription_globals;

const EMAIL_TOPIC: &str = "email";

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum EmailAction {
    Configure,
}

#[derive(Debug, Deserialize)]
struct EmailInput {
    action: EmailAction,
}

#[derive(Debug, Deserialize)]
struct ConfigureInput {
    imap_host: String,
    #[serde(default)]
    imap_port: u16,
    smtp_host: String,
    #[serde(default)]
    smtp_port: u16,
    username: String,
    password: String,
    from_address: String,
    #[serde(default)]
    allowed_senders: Vec<String>,
}

/// Executes the consolidated internal `Email` workflow action.
///
/// State-free: reaches the subscriber runtime through the process-global
/// `subscription_globals::manager()`, so this is safe to call from a
/// parallel-batch worker thread that holds no `&mut AppState`.
pub fn execute_email(cwd: &Path, input: Value) -> Result<String> {
    let parsed: EmailInput =
        serde_json::from_value(input.clone()).context("invalid Email input")?;
    match parsed.action {
        EmailAction::Configure => execute_email_configure(cwd, input),
    }
}

/// Executes `EmailConfigure`. Ensures the subscriber is running, then
/// sends an [`SubscriberCommand::EmailConfigure`] over its stdin.
pub fn execute_email_configure(_cwd: &Path, input: Value) -> Result<String> {
    let parsed: ConfigureInput =
        serde_json::from_value(input).context("invalid EmailConfigure input")?;
    ensure_subscriber_running()?;
    let manager = subscription_globals::manager()?;
    manager.send_command(
        EMAIL_TOPIC,
        &SubscriberCommand::EmailConfigure {
            imap_host: parsed.imap_host,
            imap_port: parsed.imap_port,
            smtp_host: parsed.smtp_host,
            smtp_port: parsed.smtp_port,
            username: parsed.username,
            password: parsed.password,
            from_address: parsed.from_address,
            allowed_senders: parsed.allowed_senders,
        },
    )?;
    Ok(json!({
        "status": "configured",
        "next": "Email connector is configured. Use ConnectionCreate with connector_slug=\"email\", then WorkflowCreate with that connection_slug."
    })
    .to_string())
}

fn ensure_subscriber_running() -> Result<()> {
    let manager = subscription_globals::manager()?;
    if manager.subscriber_ids().iter().any(|id| id == EMAIL_TOPIC) {
        return Ok(());
    }
    let dir = subscriber_manifest_dir(EMAIL_TOPIC);
    if !dir.join("manifest.toml").exists() {
        return Err(anyhow!(
            "email subscriber manifest not found at {}; install it before configuring",
            dir.display()
        ));
    }
    let manifest = Manifest::load(&dir)?;
    manager.start_subscriber(manifest)?;
    Ok(())
}

fn subscriber_manifest_dir(topic: &str) -> PathBuf {
    if let Some(home) = std::env::var_os("HOME") {
        let user = PathBuf::from(home)
            .join(".puffer")
            .join("subscribers")
            .join(topic);
        if user.join("manifest.toml").exists() {
            return user;
        }
    }
    PathBuf::from("resources/subscribers").join(topic)
}
