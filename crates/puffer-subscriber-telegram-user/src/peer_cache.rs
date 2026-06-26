//! Durable Telegram peer metadata cache.
//!
//! Contact inference runs offline from local state. This cache records peer
//! names observed while the subscriber is already connected, so the daemon can
//! use richer Telegram names without making live connector calls.

use std::collections::{BTreeSet, HashMap};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use base64::Engine as _;
use grammers_client::{
    types::{Chat, ChatMap, User},
    Client,
};
use grammers_session::PackedChat;
use grammers_tl_types as tl;
use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::state::SkillEnv;

const CACHE_VERSION: u32 = 1;
const AVATAR_MIME_TYPE: &str = "image/jpeg";
const SAVED_CONTACT_SOURCE: &str = "saved_contact";
const RECENT_DIALOG_SCAN_CURSOR_FILE: &str = "recent-dialog-scan-cursor.json";
const TELEGRAM_DIALOG_PAGE_LIMIT_MAX: usize = 100;

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

    /// Returns the best cached display title for one peer.
    pub(crate) fn title_for(&self, kind: &str, numeric_id: i64) -> Option<String> {
        self.find(kind, numeric_id)
            .and_then(|record| record.title.clone())
    }

    /// Returns the best cached public username for one peer.
    pub(crate) fn username_for(&self, kind: &str, numeric_id: i64) -> Option<String> {
        self.find(kind, numeric_id).and_then(|record| {
            record
                .username
                .clone()
                .or_else(|| record.usernames.first().cloned())
        })
    }

    fn find(&self, kind: &str, numeric_id: i64) -> Option<&TelegramPeerRecord> {
        self.peers
            .iter()
            .find(|record| record.kind == kind && record.numeric_id == numeric_id)
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
pub(crate) async fn hydrate_chat_avatars_deferred(
    env: &SkillEnv,
    client: &Client,
    chats: Vec<Chat>,
) {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RecentDialogPeerCacheHydration {
    pub direct_users_seen: usize,
    pub dialogs_seen: usize,
    pub batch_dialogs_seen: usize,
    pub target_direct_users: usize,
    pub max_dialogs: usize,
    pub dialogs_exhausted: bool,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq, Eq, Serialize)]
struct RecentDialogScanCursor {
    #[serde(default)]
    dialogs_seen: usize,
    #[serde(default)]
    direct_users_seen: usize,
    #[serde(default)]
    offset_date: i32,
    #[serde(default)]
    offset_id: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    offset_peer: Option<String>,
}

impl RecentDialogScanCursor {
    fn has_offset(&self) -> bool {
        self.offset_packed_peer().is_some()
    }

    fn offset_input_peer(&self) -> tl::enums::InputPeer {
        self.offset_packed_peer()
            .map(|peer| peer.to_input_peer())
            .unwrap_or(tl::enums::InputPeer::Empty)
    }

    fn offset_packed_peer(&self) -> Option<PackedChat> {
        self.offset_peer
            .as_deref()
            .and_then(|value| PackedChat::from_hex(value).ok())
    }

    fn sync_counts(&mut self, hydration: &RecentDialogPeerCacheHydration) {
        self.dialogs_seen = hydration.dialogs_seen;
        self.direct_users_seen = hydration.direct_users_seen;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RecentDialogMessageAnchor {
    offset_date: i32,
    offset_id: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct RecentDialogCursorAdvance {
    direct_user_target: bool,
    offset_peer: String,
    last_message: Option<RecentDialogMessageAnchor>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RecentDialogHydrationStep {
    Dialog { direct_user_target: bool },
    Exhausted,
    Failed,
}

fn recent_dialog_hydration_state_from_cursor(
    target_direct_users: usize,
    max_dialogs: usize,
    cursor: &RecentDialogScanCursor,
) -> RecentDialogPeerCacheHydration {
    let target_direct_users = target_direct_users.max(1);
    let max_dialogs = max_dialogs.max(1);
    RecentDialogPeerCacheHydration {
        direct_users_seen: cursor.direct_users_seen,
        dialogs_seen: cursor.dialogs_seen,
        batch_dialogs_seen: 0,
        target_direct_users,
        max_dialogs,
        dialogs_exhausted: false,
    }
}

fn recent_dialog_hydration_should_continue(hydration: &RecentDialogPeerCacheHydration) -> bool {
    hydration.batch_dialogs_seen < hydration.max_dialogs
        && hydration.direct_users_seen < hydration.target_direct_users
}

fn apply_recent_dialog_hydration_step(
    hydration: &mut RecentDialogPeerCacheHydration,
    step: RecentDialogHydrationStep,
) -> bool {
    match step {
        RecentDialogHydrationStep::Dialog { direct_user_target } => {
            hydration.dialogs_seen += 1;
            hydration.batch_dialogs_seen += 1;
            if direct_user_target {
                hydration.direct_users_seen += 1;
            }
            true
        }
        RecentDialogHydrationStep::Exhausted => {
            hydration.dialogs_exhausted = true;
            false
        }
        RecentDialogHydrationStep::Failed => false,
    }
}

fn apply_recent_dialog_cursor_advance(
    hydration: &mut RecentDialogPeerCacheHydration,
    cursor: &mut RecentDialogScanCursor,
    advance: RecentDialogCursorAdvance,
) -> bool {
    let should_continue = apply_recent_dialog_hydration_step(
        hydration,
        RecentDialogHydrationStep::Dialog {
            direct_user_target: advance.direct_user_target,
        },
    );
    cursor.sync_counts(hydration);
    if let Some(last_message) = advance.last_message {
        cursor.offset_date = last_message.offset_date;
        cursor.offset_id = last_message.offset_id;
    }
    cursor.offset_peer = Some(advance.offset_peer);
    should_continue
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

#[derive(Debug, Clone)]
struct RecentDialogBatch {
    dialogs: Vec<RecentDialogBatchDialog>,
    exhausted: bool,
}

#[derive(Debug, Clone)]
struct RecentDialogBatchDialog {
    chat: Chat,
    last_message_at_ms: Option<i64>,
    cursor_advance: RecentDialogCursorAdvance,
}

#[derive(Debug, Clone, Copy, Hash, PartialEq, Eq)]
enum RecentDialogPeerKey {
    User(i64),
    Chat(i64),
    Channel(i64),
}

impl RecentDialogPeerKey {
    fn from_peer(peer: &tl::enums::Peer) -> Self {
        match peer {
            tl::enums::Peer::User(user) => Self::User(user.user_id),
            tl::enums::Peer::Chat(chat) => Self::Chat(chat.chat_id),
            tl::enums::Peer::Channel(channel) => Self::Channel(channel.channel_id),
        }
    }
}

async fn fetch_recent_dialog_batch(
    client: &Client,
    cursor: &RecentDialogScanCursor,
    limit: usize,
) -> anyhow::Result<RecentDialogBatch> {
    let limit = limit.clamp(1, TELEGRAM_DIALOG_PAGE_LIMIT_MAX);
    let request = tl::functions::messages::GetDialogs {
        exclude_pinned: cursor.has_offset(),
        folder_id: None,
        offset_date: cursor.offset_date,
        offset_id: cursor.offset_id,
        offset_peer: cursor.offset_input_peer(),
        limit: limit as i32,
        hash: 0,
    };
    let response = client
        .invoke(&request)
        .await
        .context("fetch Telegram recent dialogs")?;
    match response {
        tl::enums::messages::Dialogs::Dialogs(dialogs) => build_recent_dialog_batch(
            dialogs.dialogs,
            dialogs.messages,
            dialogs.users,
            dialogs.chats,
            true,
        ),
        tl::enums::messages::Dialogs::Slice(dialogs) => {
            let exhausted = dialogs.dialogs.len() < limit;
            build_recent_dialog_batch(
                dialogs.dialogs,
                dialogs.messages,
                dialogs.users,
                dialogs.chats,
                exhausted,
            )
        }
        tl::enums::messages::Dialogs::NotModified(_) => Ok(RecentDialogBatch {
            dialogs: Vec::new(),
            exhausted: true,
        }),
    }
}

fn build_recent_dialog_batch(
    dialogs: Vec<tl::enums::Dialog>,
    messages: Vec<tl::enums::Message>,
    users: Vec<tl::enums::User>,
    chats: Vec<tl::enums::Chat>,
    exhausted: bool,
) -> anyhow::Result<RecentDialogBatch> {
    let chats = ChatMap::new(users, chats);
    let messages_by_peer = messages
        .into_iter()
        .filter_map(recent_dialog_message_anchor_from_raw)
        .collect::<HashMap<_, _>>();
    let mut batch_dialogs = Vec::with_capacity(dialogs.len());
    for dialog in dialogs {
        let peer = dialog.peer();
        let peer_key = RecentDialogPeerKey::from_peer(&peer);
        let Some(chat) = chats.get(&peer).cloned() else {
            anyhow::bail!("Telegram recent dialog response referenced an unknown peer");
        };
        let last_message = messages_by_peer.get(&peer_key).copied();
        let last_message_at_ms = last_message.map(|message| i64::from(message.offset_date) * 1000);
        let direct_user_target = matches!(
            &chat,
            Chat::User(user)
                if is_recent_dialog_target_user(
                    user.id(),
                    user.raw.bot,
                    last_message_at_ms.is_some()
                )
        );
        batch_dialogs.push(RecentDialogBatchDialog {
            cursor_advance: RecentDialogCursorAdvance {
                direct_user_target,
                offset_peer: chat.pack().to_hex(),
                last_message,
            },
            chat,
            last_message_at_ms,
        });
    }
    Ok(RecentDialogBatch {
        dialogs: batch_dialogs,
        exhausted,
    })
}

fn recent_dialog_message_anchor_from_raw(
    message: tl::enums::Message,
) -> Option<(RecentDialogPeerKey, RecentDialogMessageAnchor)> {
    match message {
        tl::enums::Message::Message(message) => Some((
            RecentDialogPeerKey::from_peer(&message.peer_id),
            RecentDialogMessageAnchor {
                offset_date: message.date,
                offset_id: message.id,
            },
        )),
        tl::enums::Message::Service(message) => Some((
            RecentDialogPeerKey::from_peer(&message.peer_id),
            RecentDialogMessageAnchor {
                offset_date: message.date,
                offset_id: message.id,
            },
        )),
        tl::enums::Message::Empty(_) => None,
    }
}

fn load_recent_dialog_scan_cursor(env: &SkillEnv) -> RecentDialogScanCursor {
    let path = recent_dialog_scan_cursor_path(env);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return RecentDialogScanCursor::default()
        }
        Err(error) => {
            warn!(
                path = %path.display(),
                %error,
                "failed to read Telegram recent dialog scan cursor; restarting scan"
            );
            return RecentDialogScanCursor::default();
        }
    };
    if raw.trim().is_empty() {
        return RecentDialogScanCursor::default();
    }
    match serde_json::from_str(&raw) {
        Ok(cursor) if recent_dialog_scan_cursor_is_valid(&cursor) => cursor,
        Ok(_) => {
            warn!(
                path = %path.display(),
                "Telegram recent dialog scan cursor is incomplete; restarting scan"
            );
            RecentDialogScanCursor::default()
        }
        Err(error) => {
            warn!(
                path = %path.display(),
                %error,
                "failed to parse Telegram recent dialog scan cursor; restarting scan"
            );
            RecentDialogScanCursor::default()
        }
    }
}

fn recent_dialog_scan_cursor_is_valid(cursor: &RecentDialogScanCursor) -> bool {
    if cursor.dialogs_seen == 0 && cursor.direct_users_seen == 0 && cursor.offset_peer.is_none() {
        return true;
    }
    cursor.has_offset()
}

fn save_recent_dialog_scan_cursor(
    env: &SkillEnv,
    cursor: &RecentDialogScanCursor,
) -> anyhow::Result<()> {
    let path = recent_dialog_scan_cursor_path(env);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).with_context(|| {
            format!(
                "create Telegram recent dialog cursor parent {}",
                parent.display()
            )
        })?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec_pretty(cursor)?)
        .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

fn remove_recent_dialog_scan_cursor(env: &SkillEnv) -> anyhow::Result<()> {
    let path = recent_dialog_scan_cursor_path(env);
    match std::fs::remove_file(&path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error).with_context(|| format!("remove {}", path.display())),
    }
}

fn recent_dialog_scan_cursor_path(env: &SkillEnv) -> std::path::PathBuf {
    env.state_dir.join(RECENT_DIALOG_SCAN_CURSOR_FILE)
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
) -> anyhow::Result<RecentDialogPeerCacheHydration> {
    let original = TelegramPeerCache::load(env).unwrap_or_default();
    let mut cache = original.clone();
    let mut cursor = load_recent_dialog_scan_cursor(env);
    if original.peers.is_empty() && cursor != RecentDialogScanCursor::default() {
        cursor = RecentDialogScanCursor::default();
        if let Err(error) = remove_recent_dialog_scan_cursor(env) {
            warn!(%error, "failed to remove stale Telegram recent dialog scan cursor");
        }
    }
    let mut hydration =
        recent_dialog_hydration_state_from_cursor(target_direct_users, max_dialogs, &cursor);
    while recent_dialog_hydration_should_continue(&hydration) {
        let remaining_batch_dialogs = hydration.max_dialogs - hydration.batch_dialogs_seen;
        let batch = match fetch_recent_dialog_batch(client, &cursor, remaining_batch_dialogs).await
        {
            Ok(batch) => batch,
            Err(error) => {
                warn!(
                    error = %error,
                    dialogs_seen = hydration.dialogs_seen,
                    "fetching recent dialogs failed during Telegram cache hydration; saving partial state"
                );
                apply_recent_dialog_hydration_step(
                    &mut hydration,
                    RecentDialogHydrationStep::Failed,
                );
                break;
            }
        };
        if batch.dialogs.is_empty() {
            apply_recent_dialog_hydration_step(
                &mut hydration,
                RecentDialogHydrationStep::Exhausted,
            );
            cursor.sync_counts(&hydration);
            break;
        }
        for dialog in batch.dialogs {
            cache.observe_chat_with_last_message_at_ms(
                &dialog.chat,
                "recent_dialog",
                dialog.last_message_at_ms,
            );
            apply_recent_dialog_cursor_advance(&mut hydration, &mut cursor, dialog.cursor_advance);
        }
        if batch.exhausted {
            apply_recent_dialog_hydration_step(
                &mut hydration,
                RecentDialogHydrationStep::Exhausted,
            );
            cursor.sync_counts(&hydration);
            break;
        }
    }
    cache.save_if_changed(env, &original)?;
    if hydration.dialogs_exhausted {
        if let Err(error) = remove_recent_dialog_scan_cursor(env) {
            warn!(%error, "failed to remove exhausted Telegram recent dialog scan cursor");
        }
    } else if cursor.has_offset() {
        if let Err(error) = save_recent_dialog_scan_cursor(env, &cursor) {
            warn!(%error, "failed to save Telegram recent dialog scan cursor");
        }
    }
    info!(
        dialogs_seen = hydration.dialogs_seen,
        batch_dialogs_seen = hydration.batch_dialogs_seen,
        direct_users_seen = hydration.direct_users_seen,
        target_direct_users = hydration.target_direct_users,
        max_dialogs = hydration.max_dialogs,
        dialogs_exhausted = hydration.dialogs_exhausted,
        "hydrated Telegram recent dialog peer cache"
    );
    Ok(hydration)
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

fn is_recent_dialog_target_user(user_id: i64, is_bot: bool, has_last_message: bool) -> bool {
    has_last_message && !is_bot && user_id != 777000
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
        apply_recent_dialog_hydration_step, is_recent_dialog_target_user, joined_name,
        merge_optional_name, phone_key, recent_dialog_hydration_should_continue,
        recent_dialog_hydration_state_from_cursor, saved_contact_usernames,
        telegram_username_from_contact_id, RecentDialogCursorAdvance, RecentDialogHydrationStep,
        RecentDialogMessageAnchor, RecentDialogScanCursor, TelegramPeerCache, TelegramPeerRecord,
        CACHE_VERSION,
    };
    use crate::state::SkillEnv;
    use std::collections::BTreeSet;

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
    fn lookup_returns_cached_title_and_username() {
        let cache = TelegramPeerCache {
            version: CACHE_VERSION,
            peers: vec![TelegramPeerRecord {
                id: "6156741935".to_string(),
                numeric_id: 6156741935,
                kind: "user".to_string(),
                title: Some("smith john".to_string()),
                username: None,
                usernames: vec!["johnsmith1847".to_string()],
                first_name: None,
                last_name: None,
                phone: None,
                avatar: None,
                is_bot: false,
                source: Some("test".to_string()),
                updated_at_ms: 1,
                last_message_at_ms: None,
            }],
        };

        assert_eq!(
            cache.title_for("user", 6156741935).as_deref(),
            Some("smith john")
        );
        assert_eq!(
            cache.username_for("user", 6156741935).as_deref(),
            Some("johnsmith1847")
        );
        assert_eq!(cache.title_for("group", 6156741935), None);
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
    fn recent_dialog_target_user_excludes_telegram_service_account() {
        assert!(is_recent_dialog_target_user(123, false, true));
        assert!(!is_recent_dialog_target_user(777000, false, true));
        assert!(!is_recent_dialog_target_user(123, true, true));
        assert!(!is_recent_dialog_target_user(123, false, false));
    }

    #[test]
    fn recent_dialog_hydration_marks_exhausted_only_after_stream_end() {
        let mut cursor = RecentDialogScanCursor::default();
        let mut hydration = recent_dialog_hydration_state_from_cursor(5, 10, &cursor);
        for _ in 0..3 {
            assert!(recent_dialog_hydration_should_continue(&hydration));
            let next_dialog = hydration.dialogs_seen + 1;
            assert!(apply_recent_dialog_cursor_advance_for_test(
                &mut hydration,
                &mut cursor,
                RecentDialogCursorAdvance {
                    direct_user_target: true,
                    offset_peer: format!("peer-{next_dialog}"),
                    last_message: Some(RecentDialogMessageAnchor {
                        offset_date: 10,
                        offset_id: next_dialog as i32,
                    }),
                },
            ));
        }

        assert!(recent_dialog_hydration_should_continue(&hydration));
        assert!(!apply_recent_dialog_hydration_step(
            &mut hydration,
            RecentDialogHydrationStep::Exhausted,
        ));

        assert_eq!(hydration.dialogs_seen, 3);
        assert_eq!(hydration.batch_dialogs_seen, 3);
        assert_eq!(hydration.direct_users_seen, 3);
        assert!(hydration.dialogs_exhausted);
    }

    #[test]
    fn recent_dialog_hydration_does_not_mark_exhausted_at_batch_cap() {
        let mut cursor = RecentDialogScanCursor::default();
        let mut hydration = recent_dialog_hydration_state_from_cursor(120, 3, &cursor);
        for _ in 0..3 {
            assert!(recent_dialog_hydration_should_continue(&hydration));
            let next_dialog = hydration.dialogs_seen + 1;
            assert!(apply_recent_dialog_cursor_advance_for_test(
                &mut hydration,
                &mut cursor,
                RecentDialogCursorAdvance {
                    direct_user_target: false,
                    offset_peer: format!("peer-{next_dialog}"),
                    last_message: Some(RecentDialogMessageAnchor {
                        offset_date: 10,
                        offset_id: next_dialog as i32,
                    }),
                },
            ));
        }

        assert!(!recent_dialog_hydration_should_continue(&hydration));
        assert_eq!(hydration.dialogs_seen, 3);
        assert_eq!(hydration.batch_dialogs_seen, 3);
        assert_eq!(hydration.direct_users_seen, 0);
        assert_eq!(cursor.dialogs_seen, 3);
        assert_eq!(cursor.offset_id, 3);
        assert!(!hydration.dialogs_exhausted);
    }

    #[test]
    fn recent_dialog_hydration_does_not_mark_exhausted_when_target_is_met() {
        let cursor = RecentDialogScanCursor::default();
        let mut hydration = recent_dialog_hydration_state_from_cursor(2, 10, &cursor);
        for _ in 0..2 {
            assert!(apply_recent_dialog_hydration_step(
                &mut hydration,
                RecentDialogHydrationStep::Dialog {
                    direct_user_target: true,
                },
            ));
        }

        assert!(!recent_dialog_hydration_should_continue(&hydration));
        assert_eq!(hydration.dialogs_seen, 2);
        assert_eq!(hydration.direct_users_seen, 2);
        assert!(!hydration.dialogs_exhausted);
    }

    #[test]
    fn recent_dialog_cursor_resumes_after_batch_boundary_without_gaps_or_duplicates() {
        let total_dialogs = 620usize;
        let batch_cap = 100usize;
        let target = 120usize;
        let mut expected_direct_dialogs = BTreeSet::from([100usize, 101, 500, 501]);
        for index in 1..=total_dialogs {
            if expected_direct_dialogs.len() == 117 {
                break;
            }
            if index % 5 == 0 || index % 7 == 0 {
                expected_direct_dialogs.insert(index);
            }
        }
        assert_eq!(expected_direct_dialogs.len(), 117);

        let mut cursor = RecentDialogScanCursor::default();
        let mut processed_dialogs = Vec::new();
        let mut direct_dialogs_seen = BTreeSet::new();
        let mut previous_cursor_offset = 0;
        let mut exhausted = false;

        while !exhausted {
            let mut hydration =
                recent_dialog_hydration_state_from_cursor(target, batch_cap, &cursor);
            let first_dialog = cursor.offset_id as usize + 1;
            let last_dialog = (first_dialog + batch_cap - 1).min(total_dialogs);
            for dialog_index in first_dialog..=last_dialog {
                assert!(recent_dialog_hydration_should_continue(&hydration));
                let direct_user_target = expected_direct_dialogs.contains(&dialog_index);
                assert!(apply_recent_dialog_cursor_advance_for_test(
                    &mut hydration,
                    &mut cursor,
                    RecentDialogCursorAdvance {
                        direct_user_target,
                        offset_peer: format!("peer-{dialog_index}"),
                        last_message: Some(RecentDialogMessageAnchor {
                            offset_date: 100_000 - dialog_index as i32,
                            offset_id: dialog_index as i32,
                        }),
                    },
                ));
                processed_dialogs.push(dialog_index);
                if direct_user_target {
                    direct_dialogs_seen.insert(dialog_index);
                }
            }

            assert!(
                cursor.offset_id > previous_cursor_offset,
                "cursor must advance monotonically across batches"
            );
            previous_cursor_offset = cursor.offset_id;

            if last_dialog == total_dialogs {
                assert!(!apply_recent_dialog_hydration_step(
                    &mut hydration,
                    RecentDialogHydrationStep::Exhausted,
                ));
                exhausted = hydration.dialogs_exhausted;
            }
        }

        assert_eq!(processed_dialogs.len(), total_dialogs);
        assert_eq!(
            processed_dialogs,
            (1..=total_dialogs).collect::<Vec<_>>(),
            "dialog cursor must not duplicate or skip batch-boundary rows"
        );
        assert!(direct_dialogs_seen.contains(&100));
        assert!(direct_dialogs_seen.contains(&101));
        assert_eq!(direct_dialogs_seen, expected_direct_dialogs);
        assert!(exhausted);
        assert_eq!(cursor.dialogs_seen, total_dialogs);
        assert_eq!(cursor.direct_users_seen, 117);
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

    fn apply_recent_dialog_cursor_advance_for_test(
        hydration: &mut super::RecentDialogPeerCacheHydration,
        cursor: &mut RecentDialogScanCursor,
        advance: RecentDialogCursorAdvance,
    ) -> bool {
        super::apply_recent_dialog_cursor_advance(hydration, cursor, advance)
    }
}
