//! Telegram outbound message and poll action helpers.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use grammers_client::{
    types::{media::Uploaded, Chat, Media},
    Client, InputMedia, InputMessage,
};
use grammers_tl_types as tl;
use puffer_subscriber_runtime::{SendMediaAttachment, SendMediaKind};
use serde_json::json;

use crate::events::emit_control;
use crate::peers::resolve_peer;
use crate::polls::{hex_encode, resolve_poll_options};
use crate::state::SkillEnv;

/// Resolves `peer` and sends text and optional attachments through Telegram.
pub(crate) async fn handle_send_message(
    env: &SkillEnv,
    client: &Client,
    peer: String,
    text: String,
    reply_to: Option<i32>,
    media: Vec<SendMediaAttachment>,
) -> anyhow::Result<()> {
    if text.trim().is_empty() && media.is_empty() {
        emit_control(
            &env.topic,
            "send_error",
            json!({ "peer": peer, "error": "message text or media is required" }),
        )?;
        return Ok(());
    }
    let resolved = match resolve_peer(client, &peer).await {
        Ok(chat) => chat,
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    if media.is_empty() {
        send_text(env, client, &peer, &resolved, &text, reply_to).await?;
    } else if media.len() == 1 {
        send_single_media(env, client, &peer, &resolved, &text, reply_to, &media[0]).await?;
    } else {
        send_album(env, client, &peer, &resolved, &text, reply_to, &media).await?;
    }
    Ok(())
}

/// Votes in a Telegram poll message by resolving user-facing selectors.
pub(crate) async fn handle_vote_poll(
    client: &Client,
    peer: String,
    message_id: i32,
    options: Vec<String>,
) -> anyhow::Result<String> {
    let resolved = resolve_peer(client, &peer).await?;
    let message = match client
        .get_messages_by_id(resolved.clone(), &[message_id])
        .await
    {
        Ok(mut messages) => messages.pop().flatten(),
        Err(error) => anyhow::bail!("Telegram message lookup failed: {error}"),
    };
    let Some(message) = message else {
        anyhow::bail!("poll message not found");
    };
    let Some(Media::Poll(poll)) = message.media() else {
        anyhow::bail!("message does not contain a poll");
    };
    let selected = resolve_poll_options(&poll, &options)?;
    let selected_hex = selected
        .iter()
        .map(|option| hex_encode(option))
        .collect::<Vec<_>>();
    client
        .invoke(&tl::functions::messages::SendVote {
            peer: resolved.pack().to_input_peer(),
            msg_id: message_id,
            options: selected,
        })
        .await?;
    Ok(format!(
        "voted in poll message {message_id} with {} option selectors ({})",
        options.len(),
        selected_hex.join(",")
    ))
}

async fn send_text(
    env: &SkillEnv,
    client: &Client,
    peer: &str,
    resolved: &Chat,
    text: &str,
    reply_to: Option<i32>,
) -> anyhow::Result<()> {
    let message = InputMessage::text(text).reply_to(reply_to);
    match client.send_message(resolved.clone(), message).await {
        Ok(_) => {
            emit_control(
                &env.topic,
                "send_complete",
                json!({
                    "peer": peer,
                    "chat_id": resolved.id(),
                    "reply_to": reply_to,
                    "bytes": text.len(),
                    "media_count": 0,
                }),
            )?;
        }
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": format!("{error}") }),
            )?;
        }
    }
    Ok(())
}

async fn send_single_media(
    env: &SkillEnv,
    client: &Client,
    peer: &str,
    resolved: &Chat,
    text: &str,
    reply_to: Option<i32>,
    attachment: &SendMediaAttachment,
) -> anyhow::Result<()> {
    let caption = attachment_caption(text, attachment, true);
    let message = match input_message_for_attachment(client, attachment, &caption, reply_to).await {
        Ok(message) => message,
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    match client.send_message(resolved.clone(), message).await {
        Ok(_) => {
            emit_send_complete(env, peer, resolved, text, reply_to, 1)?;
        }
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": format!("{error}") }),
            )?;
        }
    }
    Ok(())
}

async fn send_album(
    env: &SkillEnv,
    client: &Client,
    peer: &str,
    resolved: &Chat,
    text: &str,
    reply_to: Option<i32>,
    attachments: &[SendMediaAttachment],
) -> anyhow::Result<()> {
    let mut medias = Vec::with_capacity(attachments.len());
    for (index, attachment) in attachments.iter().enumerate() {
        let caption = attachment_caption(text, attachment, index == 0);
        match input_media_for_attachment(
            client,
            attachment,
            &caption,
            reply_to.filter(|_| index == 0),
        )
        .await
        {
            Ok(media) => medias.push(media),
            Err(error) => {
                emit_control(
                    &env.topic,
                    "send_error",
                    json!({ "peer": peer, "error": error.to_string() }),
                )?;
                return Ok(());
            }
        }
    }
    match client.send_album(resolved.clone(), medias).await {
        Ok(_) => {
            emit_send_complete(env, peer, resolved, text, reply_to, attachments.len())?;
        }
        Err(error) => {
            emit_control(
                &env.topic,
                "send_error",
                json!({ "peer": peer, "error": format!("{error}") }),
            )?;
        }
    }
    Ok(())
}

fn emit_send_complete(
    env: &SkillEnv,
    peer: &str,
    resolved: &Chat,
    text: &str,
    reply_to: Option<i32>,
    media_count: usize,
) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "send_complete",
        json!({
            "peer": peer,
            "chat_id": resolved.id(),
            "reply_to": reply_to,
            "bytes": text.len(),
            "media_count": media_count,
        }),
    )
}

async fn input_message_for_attachment(
    client: &Client,
    attachment: &SendMediaAttachment,
    caption: &str,
    reply_to: Option<i32>,
) -> anyhow::Result<InputMessage> {
    let kind = effective_media_kind(attachment);
    let mut message = InputMessage::text(caption).reply_to(reply_to);
    if let Some(mime_type) = attachment.mime_type.as_deref() {
        message = message.mime_type(mime_type);
    }
    let source = attachment_source(attachment)?;
    let mut message = if is_remote_source(&source) {
        apply_remote_message_media(message, kind, &source)
    } else {
        let path = expand_local_path(&source);
        let uploaded = client
            .upload_file(&path)
            .await
            .with_context(|| format!("upload Telegram media {}", path.display()))?;
        apply_local_message_media(message, kind, uploaded)
    };
    if let Some(thumbnail) = attachment
        .thumbnail
        .as_deref()
        .filter(|_| kind != SendMediaKind::Photo)
    {
        let thumbnail_path = expand_local_path(thumbnail);
        let uploaded = client.upload_file(&thumbnail_path).await.with_context(|| {
            format!(
                "upload Telegram media thumbnail {}",
                thumbnail_path.display()
            )
        })?;
        message = message.thumbnail(uploaded);
    }
    Ok(message)
}

async fn input_media_for_attachment(
    client: &Client,
    attachment: &SendMediaAttachment,
    caption: &str,
    reply_to: Option<i32>,
) -> anyhow::Result<InputMedia> {
    let kind = effective_media_kind(attachment);
    let mut media = InputMedia::caption(caption).reply_to(reply_to);
    if let Some(mime_type) = attachment.mime_type.as_deref() {
        media = media.mime_type(mime_type);
    }
    let source = attachment_source(attachment)?;
    let mut media = if is_remote_source(&source) {
        apply_remote_album_media(media, kind, &source)
    } else {
        let path = expand_local_path(&source);
        let uploaded = client
            .upload_file(&path)
            .await
            .with_context(|| format!("upload Telegram media {}", path.display()))?;
        apply_local_album_media(media, kind, uploaded)
    };
    if let Some(thumbnail) = attachment
        .thumbnail
        .as_deref()
        .filter(|_| kind != SendMediaKind::Photo)
    {
        let thumbnail_path = expand_local_path(thumbnail);
        let uploaded = client.upload_file(&thumbnail_path).await.with_context(|| {
            format!(
                "upload Telegram media thumbnail {}",
                thumbnail_path.display()
            )
        })?;
        media = media.thumbnail(uploaded);
    }
    Ok(media)
}

fn apply_local_message_media(
    message: InputMessage,
    kind: SendMediaKind,
    uploaded: Uploaded,
) -> InputMessage {
    match kind {
        SendMediaKind::Photo => message.photo(uploaded),
        SendMediaKind::File => message.file(uploaded),
        SendMediaKind::Auto | SendMediaKind::Document => message.document(uploaded),
    }
}

fn apply_local_album_media(
    media: InputMedia,
    kind: SendMediaKind,
    uploaded: Uploaded,
) -> InputMedia {
    match kind {
        SendMediaKind::Photo => media.photo(uploaded),
        SendMediaKind::File => media.file(uploaded),
        SendMediaKind::Auto | SendMediaKind::Document => media.document(uploaded),
    }
}

fn apply_remote_message_media(
    message: InputMessage,
    kind: SendMediaKind,
    source: &str,
) -> InputMessage {
    match kind {
        SendMediaKind::Photo => message.photo_url(source),
        SendMediaKind::Auto if looks_like_photo_source(source, None) => message.photo_url(source),
        SendMediaKind::Auto | SendMediaKind::Document | SendMediaKind::File => {
            message.document_url(source)
        }
    }
}

fn apply_remote_album_media(media: InputMedia, kind: SendMediaKind, source: &str) -> InputMedia {
    match kind {
        SendMediaKind::Photo => media.photo_url(source),
        SendMediaKind::Auto if looks_like_photo_source(source, None) => media.photo_url(source),
        SendMediaKind::Auto | SendMediaKind::Document | SendMediaKind::File => {
            media.document_url(source)
        }
    }
}

fn attachment_caption(text: &str, attachment: &SendMediaAttachment, first: bool) -> String {
    attachment.caption.clone().unwrap_or_else(|| {
        if first {
            text.to_string()
        } else {
            String::new()
        }
    })
}

fn attachment_source(attachment: &SendMediaAttachment) -> anyhow::Result<String> {
    let source = attachment.path.trim();
    if source.is_empty() {
        anyhow::bail!("media attachment requires `path`");
    }
    Ok(source.to_string())
}

fn effective_media_kind(attachment: &SendMediaAttachment) -> SendMediaKind {
    match attachment.kind.unwrap_or_default() {
        SendMediaKind::Auto
            if looks_like_photo_source(&attachment.path, attachment.mime_type.as_deref()) =>
        {
            SendMediaKind::Photo
        }
        SendMediaKind::Auto => SendMediaKind::Document,
        kind => kind,
    }
}

fn looks_like_photo_source(source: &str, mime_type: Option<&str>) -> bool {
    if let Some(mime_type) = mime_type {
        return matches!(
            mime_type,
            "image/jpeg" | "image/png" | "image/webp" | "image/heic" | "image/heif"
        );
    }
    let clean_source = source
        .split(['?', '#'])
        .next()
        .unwrap_or(source)
        .to_lowercase();
    matches!(
        Path::new(&clean_source)
            .extension()
            .and_then(|value| value.to_str()),
        Some("jpg" | "jpeg" | "png" | "webp" | "heic" | "heif")
    )
}

fn is_remote_source(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}

fn expand_local_path(source: &str) -> PathBuf {
    if source == "~" {
        return dirs::home_dir().unwrap_or_else(|| PathBuf::from(source));
    }
    if let Some(rest) = source.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    PathBuf::from(source)
}
