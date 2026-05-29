//! Telegram peer lookup, message search, and outbound send helpers.

use std::path::{Path, PathBuf};

use anyhow::Context as _;
use grammers_client::{
    types::media::{Contact, Document, Sticker},
    types::{Chat, Dialog, Media, Message},
    Client,
};
use puffer_subscriber_runtime::TelegramPeerKind;
use serde_json::json;

use crate::events::emit_control;
use crate::notifications::{dialog_notifications_suppressed, fetch_chat_notification_suppressed};
use crate::polls::{poll_payload, poll_text};
use crate::reply::{reply_header_payload, reply_to_label};
use crate::state::SkillEnv;

const DEFAULT_PEER_LIMIT: usize = 50;
const MAX_PEER_LIMIT: usize = 200;
const DEFAULT_MESSAGE_LIMIT: usize = 10;
const MAX_MESSAGE_LIMIT: usize = 50;
const DEFAULT_FILTERED_MESSAGE_SCAN_LIMIT: usize = 200;
const MAX_MESSAGE_SCAN_LIMIT: usize = 500;
const DEFAULT_CONTEXT_RADIUS: usize = 2;
const MAX_CONTEXT_RADIUS: usize = 10;
const MAX_MESSAGE_TEXT_CHARS: usize = 2_000;

/// Lists Telegram peers visible in the authenticated account's dialog list.
pub(crate) async fn handle_list_peers(
    env: &SkillEnv,
    client: &Client,
    query: Option<String>,
    peer_kind: Option<TelegramPeerKind>,
    limit: Option<usize>,
) -> anyhow::Result<()> {
    let limit = clamp_peer_limit(limit);
    let normalized_query = query
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(normalize_peer_text);
    let mut peers = Vec::new();
    let mut iter = client.iter_dialogs();
    while let Some(dialog) = iter
        .next()
        .await
        .with_context(|| "iter_dialogs failed while listing Telegram peers")?
    {
        let chat = dialog.chat();
        if !peer_kind_matches(peer_kind, chat) {
            continue;
        }
        if let Some(query) = &normalized_query {
            if !peer_matches_query(chat, query) {
                continue;
            }
        }
        let notification_suppressed = resolve_peer_notification_suppressed(client, &dialog).await;
        peers.push(peer_payload_with_notifications(
            chat,
            notification_suppressed,
        ));
        if peers.len() >= limit {
            break;
        }
    }
    let count = peers.len();
    emit_control(
        &env.topic,
        "peer_list",
        json!({
            "query": query,
            "peer_kind": peer_kind.map(peer_kind_filter_label),
            "limit": limit,
            "count": count,
            "limit_reached": count >= limit,
            "peers": peers,
        }),
    )?;
    Ok(())
}

async fn resolve_peer_notification_suppressed(client: &Client, dialog: &Dialog) -> bool {
    let fallback = dialog_notifications_suppressed(dialog);
    match fetch_chat_notification_suppressed(client, dialog.chat()).await {
        Ok(suppressed) => suppressed,
        Err(error) => {
            tracing::warn!(
                chat = %dialog.chat().id(),
                %error,
                "failed to refresh Telegram peer notification settings; using dialog state"
            );
            fallback
        }
    }
}

/// Searches message text inside one Telegram peer and returns nearby context.
pub(crate) async fn handle_search_messages(
    env: &SkillEnv,
    client: &Client,
    peer: String,
    query: String,
    limit: Option<usize>,
    context_radius: Option<usize>,
    succinct: bool,
) -> anyhow::Result<()> {
    let trimmed_query = query.trim();
    if trimmed_query.is_empty() {
        emit_control(
            &env.topic,
            "message_search_error",
            json!({ "peer": peer, "query": query, "error": "message search query must not be empty" }),
        )?;
        return Ok(());
    }
    let chat = match resolve_peer(client, &peer).await {
        Ok(chat) => chat,
        Err(error) => {
            emit_control(
                &env.topic,
                "message_search_error",
                json!({ "peer": peer, "query": query, "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    let limit = clamp_message_limit(limit);
    let context_radius = clamp_context_radius(context_radius);
    let mut results = Vec::new();
    let mut search = client.search_messages(chat.pack()).query(trimmed_query);
    while results.len() < limit {
        let maybe_message = match search.next().await {
            Ok(message) => message,
            Err(error) => {
                emit_control(
                    &env.topic,
                    "message_search_error",
                    json!({ "peer": peer, "query": query, "error": format!("Telegram message search failed: {error}") }),
                )?;
                return Ok(());
            }
        };
        let Some(message) = maybe_message else {
            break;
        };
        let (context, context_error) =
            match collect_message_context(client, &chat, &message, context_radius).await {
                Ok(context) => (context, None),
                Err(error) => (
                    vec![message.clone()],
                    Some(format!("Telegram context lookup failed: {error}")),
                ),
            };
        results.push(
            message_search_result_payload(env, &message, &context, context_error, succinct).await,
        );
    }
    let count = results.len();
    let chat_payload = if succinct {
        peer_summary_payload(&chat)
    } else {
        peer_payload(&chat)
    };
    emit_control(
        &env.topic,
        "message_search",
        json!({
            "format": if succinct { "succinct" } else { "full" },
            "peer": peer,
            "query": query,
            "limit": limit,
            "count": count,
            "limit_reached": count >= limit,
            "context_radius": context_radius,
            "chat": chat_payload,
            "results": results,
        }),
    )?;
    Ok(())
}

/// Lists recent Telegram messages inside one peer without a search query.
pub(crate) async fn handle_list_messages(
    env: &SkillEnv,
    client: &Client,
    peer: String,
    limit: Option<usize>,
    before_id: Option<i32>,
    sender: Option<String>,
    scan_limit: Option<usize>,
    succinct: bool,
) -> anyhow::Result<()> {
    let chat = match resolve_peer(client, &peer).await {
        Ok(chat) => chat,
        Err(error) => {
            emit_control(
                &env.topic,
                "message_list_error",
                json!({ "peer": peer, "error": error.to_string() }),
            )?;
            return Ok(());
        }
    };
    let limit = clamp_message_limit(limit);
    let sender = sender
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty());
    let sender_filter = sender.as_deref().map(normalize_peer_text);
    let scan_limit = clamp_message_scan_limit(scan_limit, limit, sender_filter.is_some());
    let mut iter = client.iter_messages(chat.pack()).limit(scan_limit);
    if let Some(before_id) = before_id {
        iter = iter.offset_id(before_id);
    }
    let mut messages = Vec::new();
    let mut scanned = 0usize;
    let mut last_scanned_id = None;
    while scanned < scan_limit && messages.len() < limit {
        let maybe_message = match iter.next().await {
            Ok(message) => message,
            Err(error) => {
                emit_control(
                    &env.topic,
                    "message_list_error",
                    json!({ "peer": peer, "error": format!("Telegram message list failed: {error}") }),
                )?;
                return Ok(());
            }
        };
        let Some(message) = maybe_message else {
            break;
        };
        scanned += 1;
        last_scanned_id = Some(message.id());
        if let Some(query) = &sender_filter {
            if !message_sender_matches(&message, query) {
                continue;
            }
        }
        messages.push(message);
    }
    let next_before_id = last_scanned_id;
    messages.sort_by_key(Message::id);
    let mut payload_messages = Vec::with_capacity(messages.len());
    for message in &messages {
        let payload = if succinct {
            concise_message_payload(env, message, None, false).await
        } else {
            message_payload(env, message, false).await
        };
        payload_messages.push(payload);
    }
    let count = payload_messages.len();
    let chat_payload = if succinct {
        peer_summary_payload(&chat)
    } else {
        peer_payload(&chat)
    };
    emit_control(
        &env.topic,
        "message_list",
        json!({
            "format": if succinct { "succinct" } else { "full" },
            "peer": peer,
            "limit": limit,
            "count": count,
            "limit_reached": count >= limit,
            "sender_filter": sender,
            "scan_limit": scan_limit,
            "scanned": scanned,
            "scan_limit_reached": scanned >= scan_limit,
            "before_id": before_id,
            "next_before_id": next_before_id,
            "chat": chat_payload,
            "messages": payload_messages,
        }),
    )?;
    Ok(())
}

async fn collect_message_context(
    client: &Client,
    chat: &Chat,
    message: &Message,
    radius: usize,
) -> anyhow::Result<Vec<Message>> {
    if radius == 0 {
        return Ok(vec![message.clone()]);
    }
    let mut context = Vec::new();
    let mut older = client
        .iter_messages(chat.pack())
        .offset_id(message.id())
        .limit(radius);
    while let Some(item) = older.next().await? {
        context.push(item);
    }
    context.sort_by_key(Message::id);
    context.push(message.clone());
    Ok(context)
}

/// Resolves a Telegram peer string into a chat visible to the authenticated account.
///
/// Supported peer formats:
/// * `@username` resolved through `Client::resolve_username`.
/// * Numeric chat id resolved by walking the cached dialog list.
pub(crate) async fn resolve_peer(client: &Client, peer: &str) -> anyhow::Result<Chat> {
    let trimmed = peer.trim();
    if let Some(handle) = trimmed.strip_prefix('@') {
        let chat = client
            .resolve_username(handle)
            .await
            .with_context(|| format!("resolve_username `@{handle}` failed"))?;
        return chat.ok_or_else(|| anyhow::anyhow!("no chat found for @{handle}"));
    }
    if let Ok(target_id) = trimmed.parse::<i64>() {
        let mut iter = client.iter_dialogs();
        while let Some(dialog) = iter
            .next()
            .await
            .with_context(|| "iter_dialogs failed while resolving numeric peer")?
        {
            if dialog.chat().id() == target_id {
                return Ok(dialog.chat().clone());
            }
        }
        return Err(anyhow::anyhow!(
            "numeric peer {target_id} not in cached dialogs; run `telegram search-peers` first or open the chat in Telegram once"
        ));
    }
    Err(anyhow::anyhow!(
        "peer `{peer}` is not a recognized format; use @username or a numeric chat id from `telegram search-peers`"
    ))
}

fn clamp_peer_limit(limit: Option<usize>) -> usize {
    limit.unwrap_or(DEFAULT_PEER_LIMIT).clamp(1, MAX_PEER_LIMIT)
}

fn clamp_message_limit(limit: Option<usize>) -> usize {
    limit
        .unwrap_or(DEFAULT_MESSAGE_LIMIT)
        .clamp(1, MAX_MESSAGE_LIMIT)
}

fn clamp_message_scan_limit(
    scan_limit: Option<usize>,
    result_limit: usize,
    sender_filtered: bool,
) -> usize {
    let default_limit = if sender_filtered {
        DEFAULT_FILTERED_MESSAGE_SCAN_LIMIT
    } else {
        result_limit
    };
    scan_limit
        .unwrap_or(default_limit)
        .clamp(result_limit, MAX_MESSAGE_SCAN_LIMIT)
}

fn clamp_context_radius(context_radius: Option<usize>) -> usize {
    context_radius
        .unwrap_or(DEFAULT_CONTEXT_RADIUS)
        .min(MAX_CONTEXT_RADIUS)
}

fn peer_kind_matches(peer_kind: Option<TelegramPeerKind>, chat: &Chat) -> bool {
    match peer_kind {
        None => true,
        Some(TelegramPeerKind::User) => matches!(chat, Chat::User(_)),
        Some(TelegramPeerKind::Group) => matches!(chat, Chat::Group(_)),
        Some(TelegramPeerKind::Channel) => matches!(chat, Chat::Channel(_)),
    }
}

fn peer_matches_query(chat: &Chat, normalized_query: &str) -> bool {
    normalized_contains(&chat.id().to_string(), normalized_query)
        || normalized_contains(&chat_display_name(chat), normalized_query)
        || normalized_contains(chat.name(), normalized_query)
        || chat
            .username()
            .is_some_and(|username| username_matches_query(username, normalized_query))
        || chat
            .usernames()
            .iter()
            .any(|username| username_matches_query(username, normalized_query))
}

fn username_matches_query(username: &str, normalized_query: &str) -> bool {
    normalized_contains(username, normalized_query)
        || normalized_contains(&format!("@{username}"), normalized_query)
}

fn normalized_contains(value: &str, normalized_query: &str) -> bool {
    normalize_peer_text(value).contains(normalized_query)
}

fn normalize_peer_text(value: &str) -> String {
    value.trim().to_lowercase()
}

fn message_sender_matches(message: &Message, normalized_query: &str) -> bool {
    if matches!(normalized_query, "me" | "self" | "outgoing") && message.outgoing() {
        return true;
    }
    message
        .sender()
        .as_ref()
        .is_some_and(|sender| peer_matches_query(sender, normalized_query))
}

fn peer_payload(chat: &Chat) -> serde_json::Value {
    let username = chat.username().map(ToString::to_string);
    let handle = username.as_ref().map(|value| format!("@{value}"));
    let usernames = chat
        .usernames()
        .into_iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let handles = usernames
        .iter()
        .map(|value| format!("@{value}"))
        .collect::<Vec<_>>();
    json!({
        "id": chat.id().to_string(),
        "numeric_id": chat.id(),
        "kind": peer_kind_label(chat),
        "title": chat_display_name(chat),
        "username": username,
        "handle": handle,
        "usernames": usernames,
        "handles": handles,
    })
}

fn peer_payload_with_notifications(chat: &Chat, notification_muted: bool) -> serde_json::Value {
    let mut payload = peer_payload(chat);
    if let Some(object) = payload.as_object_mut() {
        object.insert("notification_muted".to_string(), json!(notification_muted));
    }
    payload
}

fn peer_summary_payload(chat: &Chat) -> serde_json::Value {
    let username = chat.username().map(ToString::to_string);
    let handle = username.as_ref().map(|value| format!("@{value}"));
    json!({
        "id": chat.id().to_string(),
        "kind": peer_kind_label(chat),
        "title": chat_display_name(chat),
        "handle": handle,
    })
}

async fn message_search_result_payload(
    env: &SkillEnv,
    message: &Message,
    context: &[Message],
    context_error: Option<String>,
    succinct: bool,
) -> serde_json::Value {
    if succinct {
        return json!({
            "match": concise_message_payload(env, message, None, true).await,
            "context": concise_context_payload(env, context, message.id()).await,
            "context_error": context_error,
        });
    }
    json!({
        "message": message_payload(env, message, true).await,
        "context": full_context_payload(env, context, message.id()).await,
        "context_error": context_error,
    })
}

async fn full_context_payload(
    env: &SkillEnv,
    context: &[Message],
    match_id: i32,
) -> Vec<serde_json::Value> {
    let mut payload = Vec::with_capacity(context.len());
    for item in context {
        payload.push(message_payload(env, item, item.id() == match_id).await);
    }
    payload
}

async fn concise_context_payload(
    env: &SkillEnv,
    context: &[Message],
    match_id: i32,
) -> Vec<serde_json::Value> {
    let match_index = context
        .iter()
        .position(|item| item.id() == match_id)
        .unwrap_or(0);
    let mut payload = Vec::with_capacity(context.len());
    for (index, item) in context.iter().enumerate() {
        let offset = index as isize - match_index as isize;
        payload.push(concise_message_payload(env, item, Some(offset), item.id() == match_id).await);
    }
    payload
}

async fn concise_message_payload(
    env: &SkillEnv,
    message: &Message,
    offset: Option<isize>,
    is_match: bool,
) -> serde_json::Value {
    let media = message_media_value(env, message).await;
    let poll = message_poll_payload(message);
    let reply_to = reply_header_payload(message.reply_header());
    let text_value = display_text(message.text(), media.as_deref());
    let text_value = display_reply_text(&text_value, reply_to.as_ref());
    let (text, text_truncated) = truncate_text(&text_value);
    json!({
        "id": message.id(),
        "offset": offset,
        "date": message.date().to_rfc3339(),
        "from": message.sender().as_ref().map(chat_display_name),
        "outgoing": message.outgoing(),
        "is_match": is_match,
        "reply_to": reply_to,
        "reply_count": message.reply_count(),
        "media": media,
        "poll": poll,
        "text": text,
        "text_truncated": text_truncated,
    })
}

async fn message_payload(env: &SkillEnv, message: &Message, is_match: bool) -> serde_json::Value {
    let sender = message.sender();
    let media = message_media_value(env, message).await;
    let poll = message_poll_payload(message);
    let reply_to = resolved_reply_payload(env, message).await;
    let (text, text_truncated) = truncate_text(message.text());
    json!({
        "id": message.id(),
        "date": message.date().to_rfc3339(),
        "outgoing": message.outgoing(),
        "is_match": is_match,
        "sender": sender.as_ref().map(peer_payload),
        "reply_to": reply_to,
        "reply_count": message.reply_count(),
        "media": media,
        "poll": poll,
        "text": text,
        "text_truncated": text_truncated,
    })
}

async fn resolved_reply_payload(env: &SkillEnv, message: &Message) -> Option<serde_json::Value> {
    let mut reply_to = reply_header_payload(message.reply_header())?;
    if reply_to.get("kind").and_then(serde_json::Value::as_str) != Some("message") {
        return Some(reply_to);
    }
    let Some(object) = reply_to.as_object_mut() else {
        return Some(reply_to);
    };
    match message.get_reply().await {
        Ok(Some(reply)) => {
            object.insert(
                "resolved_message".to_string(),
                reply_reference_payload(env, &reply).await,
            );
        }
        Ok(None) => {
            object.insert("resolved_message".to_string(), serde_json::Value::Null);
        }
        Err(error) => {
            object.insert("resolve_error".to_string(), json!(error.to_string()));
        }
    }
    Some(reply_to)
}

async fn reply_reference_payload(env: &SkillEnv, message: &Message) -> serde_json::Value {
    let sender = message.sender();
    let chat = message.chat();
    let media = message_media_value(env, message).await;
    let poll = message_poll_payload(message);
    let (text, text_truncated) = truncate_text(message.text());
    json!({
        "id": message.id(),
        "date": message.date().to_rfc3339(),
        "chat": peer_summary_payload(&chat),
        "sender": sender.as_ref().map(peer_payload),
        "outgoing": message.outgoing(),
        "media": media,
        "poll": poll,
        "text": text,
        "text_truncated": text_truncated,
    })
}

async fn message_media_value(env: &SkillEnv, message: &Message) -> Option<String> {
    let media = message.media()?;
    match downloadable_media_path(env, message, &media).await {
        Some(Ok(path)) => Some(path.display().to_string()),
        Some(Err(error)) => Some(format!("media unavailable: {error}")),
        None => textual_media_value(&media),
    }
}

async fn downloadable_media_path(
    env: &SkillEnv,
    message: &Message,
    media: &Media,
) -> Option<anyhow::Result<PathBuf>> {
    if !matches!(
        media,
        Media::Photo(_) | Media::Document(_) | Media::Sticker(_)
    ) {
        return None;
    }
    let path = media_download_path(env, message, media);
    if path.exists() {
        return Some(Ok(path));
    }
    Some(
        async {
            if let Some(parent) = path.parent() {
                tokio::fs::create_dir_all(parent)
                    .await
                    .with_context(|| format!("create media directory {}", parent.display()))?;
            }
            match message.download_media(&path).await {
                Ok(true) => Ok(path),
                Ok(false) => Err(anyhow::anyhow!("message has no downloadable media")),
                Err(error) => Err(anyhow::anyhow!(error)),
            }
        }
        .await,
    )
}

fn display_text(text: &str, media: Option<&str>) -> String {
    match (text.is_empty(), media) {
        (true, Some(value)) => value.to_string(),
        (false, Some(value)) => format!("{value} {text}"),
        _ => text.to_string(),
    }
}

fn display_reply_text(text: &str, reply_to: Option<&serde_json::Value>) -> String {
    let Some(reply_label) = reply_to.and_then(reply_to_label) else {
        return text.to_string();
    };
    if text.is_empty() {
        format!("[{reply_label}]")
    } else {
        format!("[{reply_label}] {text}")
    }
}

fn textual_media_value(media: &Media) -> Option<String> {
    match media {
        Media::Poll(poll) => Some(poll_text(poll)),
        Media::Contact(contact) => Some(match contact_name(contact) {
            Some(name) => format!("contact: {name}"),
            None => "contact".to_string(),
        }),
        Media::Geo(geo) => Some(format!("location: {}, {}", geo.latitue(), geo.longitude())),
        Media::Dice(dice) => Some(format!("dice {}: {}", dice.emoji(), dice.value())),
        Media::Venue(venue) => Some(
            match (nonempty_str(venue.title()), nonempty_str(venue.address())) {
                (Some(title), Some(address)) => format!("venue: {title} - {address}"),
                (Some(title), None) => format!("venue: {title}"),
                (None, Some(address)) => format!("venue: {address}"),
                (None, None) => "venue".to_string(),
            },
        ),
        Media::GeoLive(_) => Some("live location".to_string()),
        Media::WebPage(_) => Some("web page".to_string()),
        _ => Some("media".to_string()),
    }
}

fn message_poll_payload(message: &Message) -> Option<serde_json::Value> {
    match message.media()? {
        Media::Poll(poll) => Some(poll_payload(&poll)),
        _ => None,
    }
}

fn media_download_path(env: &SkillEnv, message: &Message, media: &Media) -> PathBuf {
    let state_dir = if env.state_dir.is_absolute() {
        env.state_dir.clone()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("."))
            .join(&env.state_dir)
    };
    state_dir
        .join("telegram-media")
        .join(message.chat().id().to_string())
        .join(media_file_name(message.id(), media))
}

fn media_file_name(message_id: i32, media: &Media) -> String {
    match media {
        Media::Photo(_) => format!("{message_id}-photo.jpg"),
        Media::Document(document) => match safe_file_name(document.name()) {
            Some(name) => format!("{message_id}-{name}"),
            None => format!(
                "{message_id}-{}.{}",
                document_media_kind(document),
                document_extension(document)
            ),
        },
        Media::Sticker(sticker) => match safe_file_name(sticker.document.name()) {
            Some(name) => format!("{message_id}-{name}"),
            None => format!("{message_id}-sticker.{}", sticker_extension(sticker)),
        },
        _ => format!("{message_id}-media.bin"),
    }
}

fn document_media_kind(document: &Document) -> &'static str {
    match document.mime_type() {
        Some(value) if value.starts_with("image/") => "image",
        Some(value) if value.starts_with("video/") => "video",
        Some(value) if value.starts_with("audio/") => "audio",
        _ => "document",
    }
}

fn document_extension(document: &Document) -> &'static str {
    match document.mime_type() {
        Some("image/jpeg") => "jpg",
        Some("image/png") => "png",
        Some("image/gif") => "gif",
        Some("image/webp") => "webp",
        Some("video/mp4") => "mp4",
        Some("audio/mpeg") => "mp3",
        Some("audio/ogg") => "ogg",
        Some("application/pdf") => "pdf",
        Some("text/plain") => "txt",
        _ => "bin",
    }
}

fn sticker_extension(sticker: &Sticker) -> &'static str {
    match sticker.document.mime_type() {
        Some("application/x-tgsticker") => "tgs",
        Some("video/webm") => "webm",
        Some("image/webp") => "webp",
        _ => "bin",
    }
}

fn contact_name(contact: &Contact) -> Option<String> {
    let name = [contact.first_name(), contact.last_name()]
        .into_iter()
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if name.is_empty() {
        None
    } else {
        Some(name)
    }
}

fn safe_file_name(value: &str) -> Option<String> {
    let file_name = Path::new(value).file_name()?.to_string_lossy();
    let mut sanitized = String::with_capacity(file_name.len());
    for ch in file_name.chars() {
        if ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_') {
            sanitized.push(ch);
        } else {
            sanitized.push('_');
        }
        if sanitized.len() >= 96 {
            break;
        }
    }
    let sanitized = sanitized.trim_matches('.').trim_matches('_').to_string();
    if sanitized.is_empty() || sanitized == "-" {
        None
    } else {
        Some(sanitized)
    }
}

fn nonempty_str(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn truncate_text(value: &str) -> (String, bool) {
    if value.chars().count() <= MAX_MESSAGE_TEXT_CHARS {
        return (value.to_string(), false);
    }
    let end = value
        .char_indices()
        .nth(MAX_MESSAGE_TEXT_CHARS)
        .map(|(idx, _)| idx)
        .unwrap_or(value.len());
    (format!("{}...", &value[..end]), true)
}

fn chat_display_name(chat: &Chat) -> String {
    match chat {
        Chat::User(user) => user.full_name(),
        Chat::Group(_) | Chat::Channel(_) => chat.name().to_string(),
    }
}

fn peer_kind_label(chat: &Chat) -> &'static str {
    match chat {
        Chat::User(_) => "user",
        Chat::Group(_) => "group",
        Chat::Channel(_) => "channel",
    }
}

fn peer_kind_filter_label(peer_kind: TelegramPeerKind) -> &'static str {
    match peer_kind {
        TelegramPeerKind::User => "user",
        TelegramPeerKind::Group => "group",
        TelegramPeerKind::Channel => "channel",
    }
}

#[cfg(test)]
mod tests {
    use super::{
        clamp_context_radius, clamp_message_limit, clamp_message_scan_limit, clamp_peer_limit,
        display_reply_text, display_text, normalized_contains, truncate_text,
        username_matches_query,
    };
    use serde_json::json;

    #[test]
    fn peer_limit_uses_default_and_bounds() {
        assert_eq!(clamp_peer_limit(None), 50);
        assert_eq!(clamp_peer_limit(Some(0)), 1);
        assert_eq!(clamp_peer_limit(Some(500)), 200);
    }

    #[test]
    fn message_limit_uses_default_and_bounds() {
        assert_eq!(clamp_message_limit(None), 10);
        assert_eq!(clamp_message_limit(Some(0)), 1);
        assert_eq!(clamp_message_limit(Some(500)), 50);
    }

    #[test]
    fn message_scan_limit_expands_for_sender_filters() {
        assert_eq!(clamp_message_scan_limit(None, 20, false), 20);
        assert_eq!(clamp_message_scan_limit(None, 20, true), 200);
        assert_eq!(clamp_message_scan_limit(Some(5), 20, true), 20);
        assert_eq!(clamp_message_scan_limit(Some(5_000), 20, true), 500);
    }

    #[test]
    fn context_radius_caps_without_raising_zero() {
        assert_eq!(clamp_context_radius(None), 2);
        assert_eq!(clamp_context_radius(Some(0)), 0);
        assert_eq!(clamp_context_radius(Some(500)), 10);
    }

    #[test]
    fn peer_query_matches_case_insensitively() {
        assert!(normalized_contains("C & Jason", "jason"));
        assert!(!normalized_contains("C & Jason", "tony"));
    }

    #[test]
    fn username_query_matches_with_or_without_at_prefix() {
        assert!(username_matches_query("hzliu", "hzliu"));
        assert!(username_matches_query("hzliu", "@hzliu"));
    }

    #[test]
    fn display_text_includes_media_values() {
        assert_eq!(display_text("", Some("/tmp/photo.jpg")), "/tmp/photo.jpg");
        assert_eq!(
            display_text("caption", Some("/tmp/photo.jpg")),
            "/tmp/photo.jpg caption"
        );
        assert_eq!(
            display_text("", Some("poll: ship it? | yes / no")),
            "poll: ship it? | yes / no"
        );
        assert_eq!(display_text("plain", None), "plain");
    }

    #[test]
    fn display_reply_text_prefixes_reply_context() {
        let reply_to = json!({
            "kind": "message",
            "message_id": 42,
            "quote_text": "previous"
        });

        assert_eq!(
            display_reply_text("current", Some(&reply_to)),
            "[reply to #42: previous] current"
        );
        assert_eq!(
            display_reply_text("", Some(&reply_to)),
            "[reply to #42: previous]"
        );
    }

    #[test]
    fn truncate_text_respects_char_boundaries() {
        let text = "a".repeat(super::MAX_MESSAGE_TEXT_CHARS + 1);
        let (truncated, did_truncate) = truncate_text(&text);
        assert!(did_truncate);
        assert!(truncated.ends_with("..."));
    }
}
