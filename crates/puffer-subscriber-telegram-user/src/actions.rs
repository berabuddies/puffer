//! Telegram-specific connector action dispatcher.

use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use anyhow::Context as _;
use grammers_client::{
    types::{Chat, InputReactions},
    Client, InputMessage,
};
use grammers_tl_types as tl;
use serde_json::{json, Value};

use crate::events::emit_control;
use crate::outbound::handle_vote_poll;
use crate::peers::resolve_peer;
use crate::state::SkillEnv;

/// Dispatches a Telegram-specific connector action carried over the subscriber
/// private `Custom { op: "telegram_act" }` transport.
pub(crate) async fn handle_telegram_act(
    env: &SkillEnv,
    client: &Client,
    args: Value,
) -> anyhow::Result<()> {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim();
    let input = args.get("input").unwrap_or(&args);
    if action.is_empty() {
        emit_action_error(env, "", None, "telegram_act requires `action`")?;
        return Ok(());
    }
    let outcome = match action {
        "vote_poll" => handle_vote_poll_action(client, input).await,
        "edit_message" => edit_message(client, input).await,
        "delete_message" | "delete_messages" => delete_messages(client, input).await,
        "forward_message" | "forward_messages" => forward_messages(client, input).await,
        "pin_message" => pin_message(client, input).await,
        "unpin_message" => unpin_message(client, input).await,
        "unpin_all_messages" => unpin_all_messages(client, input).await,
        "react" | "send_reaction" => react_to_message(client, input).await,
        "mark_read" => mark_read(client, input).await,
        "clear_mentions" => clear_mentions(client, input).await,
        "send_typing" | "send_chat_action" => send_chat_action(client, input).await,
        "join_chat" => join_chat(client, input).await,
        "leave_chat" => leave_chat(client, input).await,
        "kick_participant" => kick_participant(client, input).await,
        "ban_participant" => ban_participant(client, input).await,
        "unban_participant" => unban_participant(client, input).await,
        "invite_users" | "add_chat_users" => invite_users(client, input).await,
        "update_profile" => update_profile(client, input).await,
        "update_username" => update_username(client, input).await,
        "update_avatar" | "upload_avatar" => update_avatar(client, input).await,
        "update_group_title" | "update_group_name" => update_group_title(client, input).await,
        "update_group_username" => update_group_username(client, input).await,
        "update_group_photo" => update_group_photo(client, input).await,
        "send_story" => send_story(client, input).await,
        _ => Err(anyhow::anyhow!("unsupported Telegram action `{action}`")),
    };
    match outcome {
        Ok(summary) => emit_action_complete(env, action, input, &summary)?,
        Err(error) => emit_action_error(env, action, input_peer(input), error.to_string())?,
    }
    Ok(())
}

async fn handle_vote_poll_action(client: &Client, input: &Value) -> anyhow::Result<String> {
    let peer = required_peer(input)?;
    let message_id = required_i32(input, &["message_id", "poll_message_id", "id"])?;
    let options = poll_vote_options(input)?;
    handle_vote_poll(client, peer, message_id, options).await
}

async fn edit_message(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let message_id = required_i32(input, &["message_id", "id"])?;
    let text = input
        .get("message")
        .or_else(|| input.get("text"))
        .or_else(|| input.get("caption"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow::anyhow!("edit_message requires `message` or `text`"))?;
    client
        .edit_message(chat, message_id, InputMessage::text(text))
        .await?;
    Ok(format!("edited message {message_id}"))
}

async fn delete_messages(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let ids = message_ids(input)?;
    let count = client.delete_messages(chat, &ids).await?;
    Ok(format!("deleted {count} message references"))
}

async fn forward_messages(client: &Client, input: &Value) -> anyhow::Result<String> {
    let destination = required_str(input, &["to", "target", "channel"])?;
    let source = required_str(input, &["from", "source", "from_peer"])?;
    let destination = resolve_peer(client, destination).await?;
    let source = resolve_peer(client, source).await?;
    let ids = message_ids(input)?;
    let forwarded = client.forward_messages(destination, &ids, source).await?;
    let count = forwarded.iter().filter(|item| item.is_some()).count();
    Ok(format!("forwarded {count}/{} messages", ids.len()))
}

async fn pin_message(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let message_id = required_i32(input, &["message_id", "id"])?;
    client.pin_message(chat, message_id).await?;
    Ok(format!("pinned message {message_id}"))
}

async fn unpin_message(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let message_id = required_i32(input, &["message_id", "id"])?;
    client.unpin_message(chat, message_id).await?;
    Ok(format!("unpinned message {message_id}"))
}

async fn unpin_all_messages(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    client.unpin_all_messages(chat).await?;
    Ok("unpinned all messages".to_string())
}

async fn react_to_message(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let message_id = required_i32(input, &["message_id", "id"])?;
    let mut reactions = if bool_field(input, "remove").unwrap_or(false) {
        InputReactions::remove()
    } else if let Some(document_id) = optional_i64(input, &["custom_emoji", "document_id"])? {
        InputReactions::custom_emoji(document_id)
    } else {
        let emoji = required_str(input, &["emoji", "reaction"])?;
        InputReactions::emoticon(emoji)
    };
    if bool_field(input, "big").unwrap_or(false) {
        reactions = reactions.big();
    }
    if bool_field(input, "add_to_recent").unwrap_or(false) {
        reactions = reactions.add_to_recent();
    }
    client.send_reactions(chat, message_id, reactions).await?;
    Ok(format!("updated reaction on message {message_id}"))
}

async fn mark_read(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    client.mark_as_read(chat).await?;
    Ok("marked chat as read".to_string())
}

async fn clear_mentions(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    client.clear_mentions(chat).await?;
    Ok("cleared mentions".to_string())
}

async fn send_chat_action(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let action = input
        .get("action_kind")
        .or_else(|| input.get("typing"))
        .or_else(|| input.get("chat_action"))
        .and_then(Value::as_str)
        .unwrap_or("typing");
    client
        .action(chat)
        .oneshot(send_message_action(action)?)
        .await?;
    Ok(format!("sent chat action `{action}`"))
}

async fn join_chat(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    if chat.pack().try_to_input_channel().is_none() {
        anyhow::bail!("join_chat requires a public group or channel peer");
    }
    client.join_chat(chat).await?;
    Ok("joined chat".to_string())
}

async fn leave_chat(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    client.delete_dialog(chat).await?;
    Ok("left chat".to_string())
}

async fn kick_participant(client: &Client, input: &Value) -> anyhow::Result<String> {
    let (chat, user) = chat_and_user(client, input).await?;
    client.kick_participant(chat, user).await?;
    Ok("kicked participant".to_string())
}

async fn ban_participant(client: &Client, input: &Value) -> anyhow::Result<String> {
    let (chat, user) = chat_and_user(client, input).await?;
    client
        .set_banned_rights(chat, user)
        .view_messages(false)
        .await?;
    Ok("banned participant".to_string())
}

async fn unban_participant(client: &Client, input: &Value) -> anyhow::Result<String> {
    let (chat, user) = chat_and_user(client, input).await?;
    client.set_banned_rights(chat, user).await?;
    Ok("unbanned participant".to_string())
}

async fn invite_users(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    if chat.pack().try_to_input_channel().is_none() && chat.pack().try_to_chat_id().is_none() {
        anyhow::bail!("invite_users requires a group, megagroup, or channel peer");
    }
    let users = user_refs(input)?;
    for user in &users {
        let user = resolve_peer(client, user).await?;
        if let Some(channel) = chat.pack().try_to_input_channel() {
            client
                .invoke(&tl::functions::channels::InviteToChannel {
                    channel,
                    users: vec![user.pack().to_input_user_lossy()],
                })
                .await?;
        } else if let Some(chat_id) = chat.pack().try_to_chat_id() {
            client
                .invoke(&tl::functions::messages::AddChatUser {
                    chat_id,
                    user_id: user.pack().to_input_user_lossy(),
                    fwd_limit: 100,
                })
                .await?;
        }
    }
    Ok(format!("invited {} users", users.len()))
}

async fn update_profile(client: &Client, input: &Value) -> anyhow::Result<String> {
    let first_name = optional_string(input, &["first_name"]);
    let last_name = optional_string(input, &["last_name"]);
    let about = optional_string(input, &["about", "bio"]);
    if first_name.is_none() && last_name.is_none() && about.is_none() {
        anyhow::bail!("update_profile requires first_name, last_name, about, or bio");
    }
    client
        .invoke(&tl::functions::account::UpdateProfile {
            first_name,
            last_name,
            about,
        })
        .await?;
    Ok("updated profile".to_string())
}

async fn update_username(client: &Client, input: &Value) -> anyhow::Result<String> {
    let username = required_str(input, &["username", "handle"])?;
    client
        .invoke(&tl::functions::account::UpdateUsername {
            username: username.trim_start_matches('@').to_string(),
        })
        .await?;
    Ok("updated username".to_string())
}

async fn update_avatar(client: &Client, input: &Value) -> anyhow::Result<String> {
    let path = media_path(input)?;
    let uploaded = client
        .upload_file(expand_local_path(path))
        .await
        .with_context(|| format!("upload avatar media {path}"))?;
    client
        .invoke(&tl::functions::photos::UploadProfilePhoto {
            fallback: bool_field(input, "fallback").unwrap_or(false),
            bot: None,
            file: Some(uploaded.raw),
            video: None,
            video_start_ts: None,
            video_emoji_markup: None,
        })
        .await?;
    Ok("updated avatar".to_string())
}

async fn update_group_title(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let title = required_str(input, &["title", "name"])?;
    if let Some(channel) = chat.pack().try_to_input_channel() {
        client
            .invoke(&tl::functions::channels::EditTitle {
                channel,
                title: title.to_string(),
            })
            .await?;
    } else if let Some(chat_id) = chat.pack().try_to_chat_id() {
        client
            .invoke(&tl::functions::messages::EditChatTitle {
                chat_id,
                title: title.to_string(),
            })
            .await?;
    } else {
        anyhow::bail!("update_group_title requires a group, megagroup, or channel peer");
    }
    Ok(format!("updated group title to `{title}`"))
}

async fn update_group_username(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let username = required_str(input, &["username", "handle"])?;
    let Some(channel) = chat.pack().try_to_input_channel() else {
        anyhow::bail!("update_group_username requires a channel or megagroup peer");
    };
    client
        .invoke(&tl::functions::channels::UpdateUsername {
            channel,
            username: username.trim_start_matches('@').to_string(),
        })
        .await?;
    Ok("updated group username".to_string())
}

async fn update_group_photo(client: &Client, input: &Value) -> anyhow::Result<String> {
    let chat = resolve_required_chat(client, input).await?;
    let photo = if bool_field(input, "remove").unwrap_or(false) {
        tl::enums::InputChatPhoto::Empty
    } else {
        let path = media_path(input)?;
        let uploaded = client
            .upload_file(expand_local_path(path))
            .await
            .with_context(|| format!("upload group photo {path}"))?;
        tl::types::InputChatUploadedPhoto {
            file: Some(uploaded.raw),
            video: None,
            video_start_ts: None,
            video_emoji_markup: None,
        }
        .into()
    };
    if let Some(channel) = chat.pack().try_to_input_channel() {
        client
            .invoke(&tl::functions::channels::EditPhoto { channel, photo })
            .await?;
    } else if let Some(chat_id) = chat.pack().try_to_chat_id() {
        client
            .invoke(&tl::functions::messages::EditChatPhoto { chat_id, photo })
            .await?;
    } else {
        anyhow::bail!("update_group_photo requires a group, megagroup, or channel peer");
    }
    Ok("updated group photo".to_string())
}

async fn send_story(client: &Client, input: &Value) -> anyhow::Result<String> {
    let peer = story_peer(client, input).await?;
    let path = media_path(input)?;
    let uploaded = client
        .upload_file(expand_local_path(path))
        .await
        .with_context(|| format!("upload story media {path}"))?;
    let media = if media_kind(input) == "document" {
        story_document_media(uploaded.raw, path, input)
    } else {
        tl::types::InputMediaUploadedPhoto {
            spoiler: bool_field(input, "spoiler").unwrap_or(false),
            file: uploaded.raw,
            stickers: None,
            ttl_seconds: None,
        }
        .into()
    };
    client
        .invoke(&tl::functions::stories::SendStory {
            pinned: bool_field(input, "pinned").unwrap_or(false),
            noforwards: bool_field(input, "noforwards").unwrap_or(false),
            fwd_modified: false,
            peer,
            media,
            media_areas: None,
            caption: optional_string(input, &["caption", "message", "text"]),
            entities: None,
            privacy_rules: story_privacy_rules(input),
            random_id: random_id(),
            period: optional_i32(input, &["period", "period_seconds"])?,
            fwd_from_id: None,
            fwd_from_story: None,
        })
        .await?;
    Ok("sent story".to_string())
}

fn emit_action_complete(
    env: &SkillEnv,
    action: &str,
    input: &Value,
    summary: &str,
) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "telegram_act_complete",
        json!({
            "action": action,
            "peer": input_peer(input),
            "summary": summary,
        }),
    )
}

fn emit_action_error(
    env: &SkillEnv,
    action: &str,
    peer: Option<String>,
    error: impl ToString,
) -> anyhow::Result<()> {
    emit_control(
        &env.topic,
        "telegram_act_error",
        json!({
            "action": action,
            "peer": peer,
            "error": error.to_string(),
        }),
    )
}

async fn resolve_required_chat(client: &Client, input: &Value) -> anyhow::Result<Chat> {
    let peer = required_peer(input)?;
    resolve_peer(client, &peer).await
}

async fn chat_and_user(client: &Client, input: &Value) -> anyhow::Result<(Chat, Chat)> {
    let chat = resolve_required_chat(client, input).await?;
    let user = required_str(input, &["user", "participant", "member", "user_id"])?;
    let user = resolve_peer(client, user).await?;
    Ok((chat, user))
}

async fn story_peer(client: &Client, input: &Value) -> anyhow::Result<tl::enums::InputPeer> {
    match input_peer(input).as_deref() {
        None | Some("self") | Some("me") => Ok(tl::enums::InputPeer::PeerSelf),
        Some(peer) => Ok(resolve_peer(client, peer).await?.pack().to_input_peer()),
    }
}

fn story_privacy_rules(input: &Value) -> Vec<tl::enums::InputPrivacyRule> {
    match optional_string(input, &["privacy"])
        .map(|value| value.trim().to_ascii_lowercase())
        .as_deref()
    {
        Some("contacts") => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowContacts],
        Some("close_friends") | Some("close-friends") | Some("friends") => {
            vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowCloseFriends]
        }
        Some("premium") => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowPremium],
        _ => vec![tl::enums::InputPrivacyRule::InputPrivacyValueAllowAll],
    }
}

fn send_message_action(action: &str) -> anyhow::Result<tl::enums::SendMessageAction> {
    let progress = 0;
    Ok(match action {
        "typing" => tl::enums::SendMessageAction::SendMessageTypingAction,
        "upload_photo" | "photo" => tl::types::SendMessageUploadPhotoAction { progress }.into(),
        "upload_video" | "video" => tl::types::SendMessageUploadVideoAction { progress }.into(),
        "upload_audio" | "audio" => tl::types::SendMessageUploadAudioAction { progress }.into(),
        "upload_document" | "document" | "file" => {
            tl::types::SendMessageUploadDocumentAction { progress }.into()
        }
        other => anyhow::bail!("unsupported chat action `{other}`"),
    })
}

fn story_document_media(
    file: tl::enums::InputFile,
    path: &str,
    input: &Value,
) -> tl::enums::InputMedia {
    tl::types::InputMediaUploadedDocument {
        nosound_video: false,
        force_file: bool_field(input, "force_file").unwrap_or(false),
        spoiler: bool_field(input, "spoiler").unwrap_or(false),
        file,
        thumb: None,
        mime_type: optional_string(input, &["mime_type", "mime"])
            .unwrap_or_else(|| basic_mime_type(path).to_string()),
        attributes: vec![tl::types::DocumentAttributeFilename {
            file_name: file_name(path),
        }
        .into()],
        stickers: None,
        ttl_seconds: None,
    }
    .into()
}

fn required_peer(input: &Value) -> anyhow::Result<String> {
    required_str(input, &["to", "target", "channel", "chat", "peer"]).map(ToString::to_string)
}

fn input_peer(input: &Value) -> Option<String> {
    ["to", "target", "channel", "chat", "peer"]
        .iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn required_str<'a>(input: &'a Value, keys: &[&str]) -> anyhow::Result<&'a str> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow::anyhow!("missing required field `{}`", keys[0]))
}

fn required_i32(input: &Value, keys: &[&str]) -> anyhow::Result<i32> {
    optional_i32(input, keys)?
        .ok_or_else(|| anyhow::anyhow!("missing required field `{}`", keys[0]))
}

fn optional_i32(input: &Value, keys: &[&str]) -> anyhow::Result<Option<i32>> {
    let Some(value) = keys.iter().find_map(|key| input.get(*key)) else {
        return Ok(None);
    };
    if let Some(value) = value.as_i64() {
        return i32::try_from(value)
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{} is outside i32 range", keys[0]));
    }
    if let Some(value) = value.as_str() {
        return value
            .trim()
            .parse::<i32>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{} must be an integer", keys[0]));
    }
    anyhow::bail!("{} must be an integer", keys[0])
}

fn optional_i64(input: &Value, keys: &[&str]) -> anyhow::Result<Option<i64>> {
    let Some(value) = keys.iter().find_map(|key| input.get(*key)) else {
        return Ok(None);
    };
    if let Some(value) = value.as_i64() {
        return Ok(Some(value));
    }
    if let Some(value) = value.as_str() {
        return value
            .trim()
            .parse::<i64>()
            .map(Some)
            .map_err(|_| anyhow::anyhow!("{} must be an integer", keys[0]));
    }
    anyhow::bail!("{} must be an integer", keys[0])
}

fn bool_field(input: &Value, key: &str) -> Option<bool> {
    input.get(key).and_then(Value::as_bool)
}

fn optional_string(input: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| input.get(*key).and_then(Value::as_str))
        .map(ToString::to_string)
}

fn message_ids(input: &Value) -> anyhow::Result<Vec<i32>> {
    if let Some(values) = input
        .get("message_ids")
        .or_else(|| input.get("ids"))
        .and_then(Value::as_array)
    {
        return values
            .iter()
            .map(|value| value_to_i32(value, "message_ids"))
            .collect();
    }
    Ok(vec![required_i32(input, &["message_id", "id"])?])
}

fn user_refs(input: &Value) -> anyhow::Result<Vec<String>> {
    if let Some(values) = input
        .get("users")
        .or_else(|| input.get("members"))
        .and_then(Value::as_array)
    {
        let users = values
            .iter()
            .filter_map(Value::as_str)
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if !users.is_empty() {
            return Ok(users);
        }
    }
    Ok(vec![required_str(
        input,
        &["user", "participant", "member", "user_id"],
    )?
    .to_string()])
}

fn poll_vote_options(input: &Value) -> anyhow::Result<Vec<String>> {
    if let Some(values) = input.get("options").and_then(Value::as_array) {
        let options = values
            .iter()
            .map(selector_to_string)
            .collect::<anyhow::Result<Vec<_>>>()?;
        if !options.is_empty() {
            return Ok(options);
        }
    }
    for key in ["option", "answer", "answer_index", "option_hex"] {
        if let Some(value) = input.get(key) {
            return Ok(vec![selector_to_string(value)?]);
        }
    }
    anyhow::bail!("vote_poll requires option, answer, answer_index, option_hex, or options")
}

fn selector_to_string(value: &Value) -> anyhow::Result<String> {
    if let Some(value) = value.as_str() {
        return Ok(value.to_string());
    }
    if let Some(value) = value.as_i64() {
        return Ok(value.to_string());
    }
    if let Some(object) = value.as_object() {
        for key in ["index", "text", "option", "option_hex"] {
            if let Some(value) = object.get(key) {
                return selector_to_string(value);
            }
        }
    }
    anyhow::bail!("selector must be a string, integer, or object")
}

fn value_to_i32(value: &Value, label: &str) -> anyhow::Result<i32> {
    if let Some(value) = value.as_i64() {
        return i32::try_from(value).map_err(|_| anyhow::anyhow!("{label} is outside i32 range"));
    }
    if let Some(value) = value.as_str() {
        return value
            .trim()
            .parse::<i32>()
            .map_err(|_| anyhow::anyhow!("{label} must be an integer"));
    }
    anyhow::bail!("{label} must be an integer")
}

fn media_path(input: &Value) -> anyhow::Result<&str> {
    if let Some(media) = input.get("media") {
        if let Some(path) = media.as_str() {
            return Ok(path);
        }
        if let Some(path) = media
            .get("path")
            .or_else(|| media.get("file"))
            .or_else(|| media.get("url"))
            .and_then(Value::as_str)
        {
            return Ok(path);
        }
    }
    required_str(input, &["path", "file", "url"])
}

fn media_kind(input: &Value) -> &str {
    input
        .get("kind")
        .or_else(|| input.get("type"))
        .or_else(|| {
            input
                .get("media")
                .and_then(|media| media.get("kind").or_else(|| media.get("type")))
        })
        .and_then(Value::as_str)
        .unwrap_or("photo")
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

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|value| value.to_string_lossy().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "file".to_string())
}

fn basic_mime_type(path: &str) -> &'static str {
    match Path::new(path)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase()
        .as_str()
    {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "webp" => "image/webp",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        "pdf" => "application/pdf",
        "txt" => "text/plain",
        _ => "application/octet-stream",
    }
}

fn random_id() -> i64 {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|value| value.as_nanos())
        .unwrap_or_default();
    (nanos as i64) ^ ((std::process::id() as i64) << 32)
}
