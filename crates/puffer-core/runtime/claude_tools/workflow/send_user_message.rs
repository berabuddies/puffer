use crate::AppState;
use anyhow::{bail, Context, Result};
use serde_json::json;
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

use super::store::{
    load_store, messages_path, now_ms, resolve_path, save_store, team_lead_agent_id, MessageStore,
    SendUserMessageInput, StoredMessage,
};

/// Executes the Claude-compatible `SendUserMessage` workflow tool.
pub fn execute_send_user_message(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: SendUserMessageInput =
        serde_json::from_value(input).context("invalid SendUserMessage input")?;
    if !matches!(parsed.status.as_str(), "normal" | "proactive") {
        bail!("SendUserMessage status must be `normal` or `proactive`");
    }
    if parsed.message.trim().is_empty() {
        bail!("SendUserMessage message cannot be empty");
    }
    let resolved_attachments = parsed
        .attachments
        .iter()
        .map(|attachment| resolve_path(cwd, attachment))
        .collect::<Vec<_>>();
    for resolved in &resolved_attachments {
        if !resolved.exists() {
            bail!(
                "SendUserMessage attachment does not exist: {}",
                resolved.display()
            );
        }
    }
    let mut messages = load_store::<MessageStore>(&messages_path(state.session.cwd.as_path()))?;
    let from = if let Some(actor) = state.current_actor.as_ref() {
        actor.agent_id.clone().unwrap_or_else(|| actor.id.clone())
    } else if let Some(team_name) = state.active_team_name.as_deref() {
        team_lead_agent_id(team_name)
    } else {
        "assistant".to_string()
    };
    messages.messages.push(StoredMessage {
        id: format!("user-msg-{}", Uuid::new_v4().simple()),
        to: "user".to_string(),
        from,
        read: false,
        summary: Some(parsed.status.clone()),
        message: json!({
            "message": parsed.message,
            "attachments": resolved_attachments
                .iter()
                .map(|path| path.display().to_string())
                .collect::<Vec<_>>(),
            "status": parsed.status,
        }),
        actor: Some(state.assistant_actor()),
        created_at_ms: now_ms(),
    });
    save_store(&messages_path(state.session.cwd.as_path()), &messages)?;
    Ok(serde_json::to_string_pretty(
        &messages.messages.last().cloned(),
    )?)
}
