//! Durable Telegram peer metadata cache.
//!
//! Contact inference runs offline from local state. This cache records peer
//! names observed while the subscriber is already connected, so the daemon can
//! use richer Telegram names without making live connector calls.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use base64::Engine as _;
use grammers_client::{
    types::{Chat, User},
    Client,
};
use grammers_tl_types as tl;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::state::SkillEnv;

const CACHE_VERSION: u32 = 1;
const AVATAR_MIME_TYPE: &str = "image/jpeg";
const SAVED_CONTACT_SOURCE: &str = "saved_contact";

/// Durable cache of Telegram peer metadata for one subscriber account.
#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
pub(crate) struct TelegramPeerCache {
    #[serde(default)]
    version: u32,
    #[serde(default)]
    peers: Vec<TelegramPeerRecord>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Serialize)]
struct TelegramPeerRecord {
    id: String,
    numeric_id: i64,
    kind: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    username: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    usernames: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    first_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    phone: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    avatar: Option<String>,
    #[serde(default)]
    is_bot: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default)]
    updated_at_ms: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    last_message_at_ms: Option<i64>,
}

impl TelegramPeerCache {
    /// Loads the peer cache from the subscriber state directory.
    pub(crate) fn load(env: &SkillEnv) -> anyhow::Result<Self> {
        let path = peer_cache_path(env);
        if !path.exists() {
            return Ok(Self {
                version: CACHE_VERSION,
                peers: Vec::new(),
            });
        }
        let raw = std::fs::read_to_string(&path)
            .with_context(|| format!("read Telegram peer cache {}", path.display()))?;
        if raw.trim().is_empty() {
            return Ok(Self {
                version: CACHE_VERSION,
                peers: Vec::new(),
            });
        }
        let mut cache: Self = serde_json::from_str(&raw)
            .with_context(|| format!("parse Telegram peer cache {}", path.display()))?;
        cache.version = CACHE_VERSION;
        Ok(cache)
    }

    /// Records metadata for one Telegram chat or user.
    pub(crate) fn observe_chat(&mut self, chat: &Chat, source: &str) {
        self.observe_chat_with_avatar(chat, None, source);
    }

    /// Records metadata plus the last message timestamp observed from a dialog snapshot.
    pub(crate) fn observe_chat_with_last_message_at_ms(
        &mut self,
        chat: &Chat,
        source: &str,
        last_message_at_ms: Option<i64>,
    ) {
        if let Some(mut record) = record_from_chat(chat, source) {
            record.last_message_at_ms = last_message_at_ms;
            self.merge(record);
        }
    }

    fn observe_chat_with_avatar(&mut self, chat: &Chat, avatar: Option<String>, source: &str) {
        if let Some(mut record) = record_from_chat(chat, source) {
            record.avatar = avatar;
            self.merge(record);
        }
    }

    /// Saves the peer cache when it changed from the original loaded value.
    pub(crate) fn save_if_changed(
        &self,
        env: &SkillEnv,
        original: &TelegramPeerCache,
    ) -> anyhow::Result<()> {
        if self == original {
            return Ok(());
        }
        self.save(env)
    }

    fn observe_user(&mut self, user: &User, saved_name: Option<String>, source: &str) {
        let record = record_from_user(user, saved_name, source);
        self.merge(record);
    }

    pub(crate) fn has_avatar(&self, chat: &Chat) -> bool {
        self.peers.iter().any(|record| {
            record.numeric_id == chat.id()
                && record.kind == peer_kind_label(chat)
                && record
                    .avatar
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|value| !value.is_empty())
        })
    }

    fn merge(&mut self, mut candidate: TelegramPeerRecord) {
        candidate.updated_at_ms = now_unix_millis();
        let Some(existing) = self
            .peers
            .iter_mut()
            .find(|record| record.id == candidate.id && record.kind == candidate.kind)
        else {
            self.peers.push(candidate);
            self.peers.sort_by(|left, right| {
                left.kind
                    .cmp(&right.kind)
                    .then_with(|| left.numeric_id.cmp(&right.numeric_id))
            });
            return;
        };

        merge_optional_name(&mut existing.title, candidate.title);
        merge_optional_name(&mut existing.first_name, candidate.first_name);
        merge_optional_name(&mut existing.last_name, candidate.last_name);
        merge_optional_fill(&mut existing.username, candidate.username);
        merge_optional_fill(&mut existing.phone, candidate.phone);
        if candidate.avatar.is_some() {
            existing.avatar = candidate.avatar;
        }
        existing.usernames = merged_usernames(&existing.usernames, &candidate.usernames);
        existing.is_bot |= candidate.is_bot;
        existing.source = candidate.source.or_else(|| existing.source.clone());
        existing.updated_at_ms = candidate.updated_at_ms;
        if let Some(last_message_at_ms) = candidate.last_message_at_ms {
            existing.last_message_at_ms = Some(
                existing
                    .last_message_at_ms
                    .map_or(last_message_at_ms, |current| {
                        current.max(last_message_at_ms)
                    }),
            );
        }
    }

    fn save(&self, env: &SkillEnv) -> anyhow::Result<()> {
        let path = peer_cache_path(env);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("create Telegram peer cache parent {}", parent.display())
            })?;
        }
        let tmp = path.with_extension("tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)
            .with_context(|| format!("write {}", tmp.display()))?;
        std::fs::rename(&tmp, &path)
            .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
    }
}

/// Fetches and records a small profile avatar for one Telegram chat when absent.
pub(crate) async fn hydrate_chat_avatar(
    client: &Client,
    cache: &mut TelegramPeerCache,
    chat: &Chat,
    source: &str,
) -> anyhow::Result<bool> {
    if cache.has_avatar(chat) {
        return Ok(false);
    }
    let Some(avatar) = fetch_chat_avatar_data_uri(client, chat).await? else {
        return Ok(false);
    };
    cache.observe_chat_with_avatar(chat, Some(avatar), source);
    Ok(true)
}

/// How many avatar downloads run concurrently in the deferred pass. Avatars
/// are small thumbnails; a modest fan-out stays well under Telegram's media
/// flood limits while collapsing hundreds of serial round-trips.
const DEFERRED_AVATAR_FETCH_CONCURRENCY: usize = 8;

/// Fetches avatars for `chats` concurrently and merges them into the durable
/// peer cache. Runs OFF the startup-hydration critical path: avatars only
/// feed contact-picker UI, so they must never delay live message delivery
/// (a fresh login on a large account used to spend ~2 minutes downloading
/// them serially before the update loop could start).
pub(crate) async fn hydrate_chat_avatars_deferred(env: &SkillEnv, client: &Client, chats: Vec<Chat>) {
    if chats.is_empty() {
        return;
    }
    let total = chats.len();
    let mut fetched: Vec<(Chat, String)> = Vec::new();
    let mut chats = chats.into_iter();
    let mut join_set = tokio::task::JoinSet::new();
    loop {
        while join_set.len() < DEFERRED_AVATAR_FETCH_CONCURRENCY {
            let Some(chat) = chats.next() else { break };
            let client = client.clone();
            join_set.spawn(async move {
                let avatar = fetch_chat_avatar_data_uri(&client, &chat).await;
                (chat, avatar)
            });
        }
        let Some(result) = join_set.join_next().await else {
            break;
        };
        match result {
            Ok((chat, Ok(Some(avatar)))) => fetched.push((chat, avatar)),
            Ok((_, Ok(None))) => {}
            Ok((chat, Err(error))) => {
                warn!(
                    chat = %chat.id(),
                    %error,
                    "failed to fetch Telegram avatar in deferred hydration"
                );
            }
            Err(error) => {
                warn!(%error, "deferred Telegram avatar fetch task failed");
            }
        }
    }
    // Reload before merging: the daemon's contact-picker hydrations may have
    // written the cache while the downloads ran.
    let original = TelegramPeerCache::load(env).unwrap_or_default();
    let mut cache = original.clone();
    for (chat, avatar) in &fetched {
        cache.observe_chat_with_avatar(chat, Some(avatar.clone()), "dialog");
    }
    if let Err(error) = cache.save_if_changed(env, &original) {
        warn!(%error, "failed to save deferred Telegram avatar hydration");
        return;
    }
    info!(
        total,
        hydrated = fetched.len(),
        "hydrated Telegram dialog avatars in background"
    );
}

/// Hydrates the peer cache from Telegram's contact book response.
pub(crate) async fn hydrate_contact_book(
    client: &Client,
    cache: &mut TelegramPeerCache,
) -> anyhow::Result<()> {
    let saved_names = saved_phone_contact_names(client).await;
    let response = client
        .invoke(&tl::functions::contacts::GetContacts { hash: 0 })
        .await
        .context("fetch Telegram contacts")?;
    let tl::enums::contacts::Contacts::Contacts(contacts) = response else {
        return Ok(());
    };
    for raw_user in contacts.users {
        let user = User::from_raw(raw_user);
        let saved_name = user
            .phone()
            .and_then(|phone| saved_names.get(&phone_key(phone)).cloned());
        cache.observe_user(&user, saved_name, "contacts");
    }
    Ok(())
}

/// Hydrates and saves the durable peer cache from Telegram's contact book.
///
/// This is intentionally narrower than subscriber startup hydration: callers
/// that only need contact-pickers can populate direct-user metadata without
/// starting the live update subscriber or monitor pipeline.
pub async fn hydrate_contact_book_cache(env: &SkillEnv, client: &Client) -> anyhow::Result<bool> {
    let original = TelegramPeerCache::load(env).unwrap_or_default();
    let mut cache = original.clone();
    hydrate_contact_book(client, &mut cache).await?;
    let changed = cache != original;
    cache.save_if_changed(env, &original)?;
    Ok(changed)
}

/// Hydrates recent direct-user dialog metadata without starting the subscriber.
///
/// This is used by onboarding/contact pickers before a monitor exists. It
/// records dialog names and last-message timestamps only; it does not mutate
/// the delivery cursor and does not emit connector events.
pub async fn hydrate_recent_dialog_peer_cache(
    env: &SkillEnv,
    client: &Client,
    target_direct_users: usize,
    max_dialogs: usize,
) -> anyhow::Result<usize> {
    let original = TelegramPeerCache::load(env).unwrap_or_default();
    let mut cache = original.clone();
    let target_direct_users = target_direct_users.max(1);
    let max_dialogs = max_dialogs.max(target_direct_users);
    let mut dialogs_seen = 0usize;
    let mut direct_users_seen = 0usize;
    let mut iter = client.iter_dialogs();
    while dialogs_seen < max_dialogs && direct_users_seen < target_direct_users {
        let dialog = match iter.next().await {
            Ok(Some(dialog)) => dialog,
            Ok(None) => break,
            Err(error) => {
                warn!(
                    error = %error,
                    dialogs_seen,
                    "iter_dialogs failed during Telegram recent dialog cache hydration; saving partial state"
                );
                break;
            }
        };
        dialogs_seen += 1;
        let chat = dialog.chat();
        let last_message_at_ms = dialog
            .last_message
            .as_ref()
            .map(|message| message.date().timestamp_millis());
        cache.observe_chat_with_last_message_at_ms(chat, "recent_dialog", last_message_at_ms);
        if matches!(chat, Chat::User(user) if !user.raw.bot) && last_message_at_ms.is_some() {
            direct_users_seen += 1;
        }
    }
    cache.save_if_changed(env, &original)?;
    info!(
        dialogs_seen,
        direct_users_seen,
        target_direct_users,
        max_dialogs,
        "hydrated Telegram recent dialog peer cache"
    );
    Ok(direct_users_seen)
}

/// Resolves saved `telegram@username` contact ids into cached Telegram peers.
pub(crate) async fn hydrate_saved_contact_usernames(
    env: &SkillEnv,
    client: &Client,
    cache: &mut TelegramPeerCache,
) -> anyhow::Result<usize> {
    let usernames = saved_contact_usernames(env)?;
    let mut resolved = 0usize;
    for username in usernames {
        let chat = match client.resolve_username(&username).await {
            Ok(Some(chat)) => chat,
            Ok(None) => {
                warn!(
                    username = %username,
                    "Telegram saved contact username did not resolve"
                );
                continue;
            }
            Err(error) => {
                warn!(
                    username = %username,
                    %error,
                    "failed to resolve Telegram saved contact username"
                );
                continue;
            }
        };
        cache.observe_chat(&chat, SAVED_CONTACT_SOURCE);
        match hydrate_chat_avatar(client, cache, &chat, SAVED_CONTACT_SOURCE).await {
            Ok(_) => {}
            Err(error) => {
                warn!(
                    username = %username,
                    chat = %chat.id(),
                    %error,
                    "failed to hydrate Telegram saved contact avatar"
                );
            }
        }
        resolved += 1;
    }
    Ok(resolved)
}

#[derive(Debug, Default, Deserialize)]
struct SavedContactStore {
    #[serde(default)]
    contacts: Vec<SavedContactRecord>,
}

#[derive(Debug, Default, Deserialize)]
struct SavedContactRecord {
    #[serde(default)]
    contact_ids: Vec<String>,
}

fn saved_contact_usernames(env: &SkillEnv) -> anyhow::Result<Vec<String>> {
    let Some(workspace_config_dir) = env.workspace_config_dir.as_ref() else {
        return Ok(Vec::new());
    };
    let path = workspace_config_dir.join("runtime").join("contacts.json");
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = std::fs::read_to_string(&path)
        .with_context(|| format!("read saved contacts {}", path.display()))?;
    if raw.trim().is_empty() {
        return Ok(Vec::new());
    }
    let store: SavedContactStore = serde_json::from_str(&raw)
        .with_context(|| format!("parse saved contacts {}", path.display()))?;
    let mut usernames = BTreeSet::new();
    for contact in store.contacts {
        for contact_id in contact.contact_ids {
            if let Some(username) = telegram_username_from_contact_id(&contact_id) {
                usernames.insert(username);
            }
        }
    }
    Ok(usernames.into_iter().collect())
}

fn telegram_username_from_contact_id(contact_id: &str) -> Option<String> {
    let username = contact_id
        .trim()
        .strip_prefix("telegram@")?
        .trim()
        .trim_start_matches('@')
        .to_ascii_lowercase();
    if telegram_username_is_public_handle(&username) {
        Some(username)
    } else {
        None
    }
}

fn telegram_username_is_public_handle(username: &str) -> bool {
    let len = username.len();
    (5..=32).contains(&len)
        && !username.chars().all(|ch| ch.is_ascii_digit())
        && username
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '_')
}

async fn saved_phone_contact_names(client: &Client) -> std::collections::HashMap<String, String> {
    let mut names = std::collections::HashMap::new();
    let response = match client.invoke(&tl::functions::contacts::GetSaved {}).await {
        Ok(response) => response,
        Err(error) => {
            warn!(
                %error,
                "failed to fetch Telegram saved contact names; using contact users only"
            );
            return names;
        }
    };
    for saved in response {
        let tl::enums::SavedContact::SavedPhoneContact(contact) = saved;
        if let Some(name) = joined_name(&contact.first_name, Some(contact.last_name.as_str())) {
            names.insert(phone_key(&contact.phone), name);
        }
    }
    names
}

fn record_from_chat(chat: &Chat, source: &str) -> Option<TelegramPeerRecord> {
    match chat {
        Chat::User(user) => Some(record_from_user(user, None, source)),
        Chat::Group(_) | Chat::Channel(_) => Some(TelegramPeerRecord {
            id: chat.id().to_string(),
            numeric_id: chat.id(),
            kind: peer_kind_label(chat).to_string(),
            title: nonempty(chat.name()),
            username: chat.username().and_then(nonempty),
            usernames: chat.usernames().into_iter().filter_map(nonempty).collect(),
            first_name: None,
            last_name: None,
            phone: None,
            avatar: None,
            is_bot: username_looks_like_bot(chat.username()),
            source: Some(source.to_string()),
            updated_at_ms: now_unix_millis(),
            last_message_at_ms: None,
        }),
    }
}

fn record_from_user(user: &User, saved_name: Option<String>, source: &str) -> TelegramPeerRecord {
    let first_name = nonempty(user.first_name());
    let last_name = user.last_name().and_then(nonempty);
    let profile_name = joined_name(user.first_name(), user.last_name());
    let title = saved_name
        .and_then(|name| nonempty(&name))
        .or(profile_name)
        .or_else(|| first_name.clone());
    let username = user.username().and_then(nonempty);
    let usernames = user
        .usernames()
        .into_iter()
        .filter_map(nonempty)
        .collect::<Vec<_>>();
    TelegramPeerRecord {
        id: user.id().to_string(),
        numeric_id: user.id(),
        kind: "user".to_string(),
        title,
        username,
        usernames,
        first_name,
        last_name,
        phone: user.phone().and_then(nonempty),
        avatar: None,
        is_bot: user.raw.bot || username_looks_like_bot(user.username()),
        source: Some(source.to_string()),
        updated_at_ms: now_unix_millis(),
        last_message_at_ms: None,
    }
}

async fn fetch_chat_avatar_data_uri(
    client: &Client,
    chat: &Chat,
) -> anyhow::Result<Option<String>> {
    let Some(downloadable) = chat.photo_downloadable(false) else {
        return Ok(None);
    };
    let mut bytes = Vec::new();
    let mut download = client.iter_download(&downloadable);
    while let Some(chunk) = download.next().await? {
        bytes.extend(chunk);
    }
    if bytes.is_empty() {
        return Ok(None);
    }
    let encoded = base64::engine::general_purpose::STANDARD.encode(bytes);
    Ok(Some(format!("data:{AVATAR_MIME_TYPE};base64,{encoded}")))
}

fn peer_cache_path(env: &SkillEnv) -> std::path::PathBuf {
    env.state_dir.join("peer-cache.json")
}

fn peer_kind_label(chat: &Chat) -> &'static str {
    match chat {
        Chat::User(_) => "user",
        Chat::Group(_) => "group",
        Chat::Channel(_) => "channel",
    }
}

fn merge_optional_name(existing: &mut Option<String>, candidate: Option<String>) {
    let Some(candidate) = candidate else {
        return;
    };
    if name_is_more_complete(existing.as_deref(), &candidate) {
        *existing = Some(candidate);
    }
}

fn merge_optional_fill(existing: &mut Option<String>, candidate: Option<String>) {
    if existing
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .is_none()
    {
        *existing = candidate;
    }
}

fn name_is_more_complete(existing: Option<&str>, candidate: &str) -> bool {
    let Some(existing) = existing.map(str::trim).filter(|value| !value.is_empty()) else {
        return true;
    };
    let candidate = candidate.trim();
    let existing_parts = existing.split_whitespace().count();
    let candidate_parts = candidate.split_whitespace().count();
    candidate_parts > existing_parts
        || (candidate_parts == existing_parts && candidate.len() > existing.len())
}

fn merged_usernames(left: &[String], right: &[String]) -> Vec<String> {
    let mut values = BTreeSet::new();
    for value in left.iter().chain(right) {
        if let Some(value) = nonempty(value) {
            values.insert(value);
        }
    }
    values.into_iter().collect()
}

fn joined_name(first_name: &str, last_name: Option<&str>) -> Option<String> {
    let mut parts = Vec::new();
    if let Some(first_name) = nonempty(first_name) {
        parts.push(first_name);
    }
    if let Some(last_name) = last_name.and_then(nonempty) {
        parts.push(last_name);
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn nonempty(value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn phone_key(value: &str) -> String {
    value.chars().filter(|ch| ch.is_ascii_digit()).collect()
}

fn username_looks_like_bot(username: Option<&str>) -> bool {
    username
        .map(|value| value.to_ascii_lowercase().ends_with("bot"))
        .unwrap_or(false)
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

#[cfg(test)]
mod tests {
    use super::{
        joined_name, merge_optional_name, phone_key, saved_contact_usernames,
        telegram_username_from_contact_id,
    };
    use crate::state::SkillEnv;

    #[test]
    fn joined_name_skips_blank_parts() {
        assert_eq!(
            joined_name("Rin", Some("Tohsaka")).as_deref(),
            Some("Rin Tohsaka")
        );
        assert_eq!(joined_name("Rin", Some("")).as_deref(), Some("Rin"));
        assert_eq!(joined_name("", None), None);
    }

    #[test]
    fn merge_optional_name_prefers_more_complete_value() {
        let mut existing = Some("Rin".to_string());

        merge_optional_name(&mut existing, Some("Rin Tohsaka".to_string()));

        assert_eq!(existing.as_deref(), Some("Rin Tohsaka"));
    }

    #[test]
    fn phone_key_keeps_only_digits() {
        assert_eq!(phone_key("+1 (555) 0100"), "15550100");
    }

    #[test]
    fn telegram_username_from_contact_id_accepts_public_handles() {
        assert_eq!(
            telegram_username_from_contact_id("telegram@Alice_42").as_deref(),
            Some("alice_42")
        );
        assert_eq!(telegram_username_from_contact_id("telegram@12345"), None);
        assert_eq!(telegram_username_from_contact_id("telegram@bad-name"), None);
        assert_eq!(telegram_username_from_contact_id("google@alice"), None);
    }

    #[test]
    fn saved_contact_usernames_reads_workspace_contacts() {
        let temp = tempfile::tempdir().unwrap();
        let workspace_config_dir = temp.path().join(".puffer");
        std::fs::create_dir_all(workspace_config_dir.join("runtime")).unwrap();
        std::fs::write(
            workspace_config_dir.join("runtime/contacts.json"),
            r#"{
              "contacts": [
                {"contact_ids": ["telegram@Alice_42", "google@alice@example.com"]},
                {"contact_ids": ["telegram@bob_user", "telegram@12345", "telegram@bad-name"]}
              ]
            }"#,
        )
        .unwrap();
        let env = SkillEnv {
            state_dir: temp.path().join("state"),
            session_path: temp.path().join("state/telegram.session"),
            topic: "telegram-user".to_string(),
            workspace_config_dir: Some(workspace_config_dir),
            live_session_path: None,
        };

        assert_eq!(
            saved_contact_usernames(&env).unwrap(),
            vec!["alice_42".to_string(), "bob_user".to_string()]
        );
    }
}
