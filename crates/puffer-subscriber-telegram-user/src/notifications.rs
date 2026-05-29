//! Telegram notification mute helpers.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::{
    types::{Chat, Dialog, Message},
    Client,
};
use grammers_tl_types as tl;
use tracing::warn;

const REMOTE_NOTIFICATION_REFRESH_INTERVAL_MS: u64 = 60_000;

/// Returns whether a dialog's current peer notification settings suppress messages.
pub(crate) fn dialog_notifications_suppressed(dialog: &Dialog) -> bool {
    dialog_notifications_suppressed_at(dialog, now_unix_seconds())
}

/// Fetches current peer notification settings and returns whether they suppress messages.
pub(crate) async fn fetch_chat_notification_suppressed(
    client: &Client,
    chat: &Chat,
) -> anyhow::Result<bool> {
    let peer = tl::types::InputNotifyPeer {
        peer: chat.pack().to_input_peer(),
    }
    .into();
    let request = tl::functions::account::GetNotifySettings { peer };
    let settings = client
        .invoke(&request)
        .await
        .with_context(|| format!("get Telegram notification settings for chat {}", chat.id()))?;
    Ok(peer_notify_settings_muted_at(&settings, now_unix_seconds()))
}

/// Tracks peer notification mute state observed from dialogs and raw updates.
#[derive(Debug, Default)]
pub(crate) struct NotificationMuteCache {
    muted_chat_ids: BTreeSet<i64>,
    remote_checked_at_ms: BTreeMap<i64, u64>,
}

impl NotificationMuteCache {
    /// Records a dialog's current notification mute state and returns whether it is muted.
    pub(crate) fn observe_dialog(&mut self, dialog: &Dialog) -> bool {
        let muted = dialog_notifications_suppressed(dialog);
        self.set_chat_muted(dialog.chat().id(), muted);
        muted
    }

    /// Returns whether the message's chat is currently muted or notification-silent.
    pub(crate) async fn message_chat_muted(&mut self, client: &Client, message: &Message) -> bool {
        let chat = message.chat();
        let chat_id = chat.id();
        if self.remote_refresh_due(chat_id) {
            match self.refresh_chat(client, &chat).await {
                Ok(muted) => return muted,
                Err(error) => {
                    warn!(
                        chat = chat_id,
                        %error,
                        "failed to refresh Telegram notification settings; using cached state"
                    );
                }
            }
        }
        self.chat_muted(chat_id)
    }

    /// Applies a raw Telegram notification-settings update when it targets one concrete peer.
    pub(crate) fn apply_raw_update(&mut self, update: &tl::enums::Update) {
        let tl::enums::Update::NotifySettings(update) = update else {
            return;
        };
        let Some(chat_id) = notify_peer_chat_id(&update.peer) else {
            return;
        };
        let muted = peer_notify_settings_muted_at(&update.notify_settings, now_unix_seconds());
        self.set_chat_muted(chat_id, muted);
        self.remote_checked_at_ms.insert(chat_id, now_unix_millis());
    }

    async fn refresh_chat(&mut self, client: &Client, chat: &Chat) -> anyhow::Result<bool> {
        let muted = fetch_chat_notification_suppressed(client, chat).await?;
        self.set_chat_muted(chat.id(), muted);
        self.remote_checked_at_ms
            .insert(chat.id(), now_unix_millis());
        Ok(muted)
    }

    fn set_chat_muted(&mut self, chat_id: i64, muted: bool) {
        if muted {
            self.muted_chat_ids.insert(chat_id);
        } else {
            self.muted_chat_ids.remove(&chat_id);
        }
    }

    fn chat_muted(&self, chat_id: i64) -> bool {
        self.muted_chat_ids.contains(&chat_id)
    }

    fn remote_refresh_due(&self, chat_id: i64) -> bool {
        match self.remote_checked_at_ms.get(&chat_id) {
            Some(checked_at) => {
                now_unix_millis().saturating_sub(*checked_at)
                    >= REMOTE_NOTIFICATION_REFRESH_INTERVAL_MS
            }
            None => true,
        }
    }
}

fn dialog_notifications_suppressed_at(dialog: &Dialog, now_seconds: i64) -> bool {
    match &dialog.raw {
        tl::enums::Dialog::Dialog(dialog) => {
            peer_notify_settings_muted_at(&dialog.notify_settings, now_seconds)
        }
        tl::enums::Dialog::Folder(_) => false,
    }
}

fn peer_notify_settings_muted_at(
    settings: &tl::enums::PeerNotifySettings,
    now_seconds: i64,
) -> bool {
    match settings {
        tl::enums::PeerNotifySettings::Settings(settings) => {
            settings.silent == Some(true)
                || settings
                    .mute_until
                    .is_some_and(|mute_until| i64::from(mute_until) > now_seconds)
        }
    }
}

fn notify_peer_chat_id(peer: &tl::enums::NotifyPeer) -> Option<i64> {
    match peer {
        tl::enums::NotifyPeer::Peer(peer) => peer_chat_id(&peer.peer),
        tl::enums::NotifyPeer::NotifyForumTopic(topic) => peer_chat_id(&topic.peer),
        tl::enums::NotifyPeer::NotifyUsers
        | tl::enums::NotifyPeer::NotifyChats
        | tl::enums::NotifyPeer::NotifyBroadcasts => None,
    }
}

fn peer_chat_id(peer: &tl::enums::Peer) -> Option<i64> {
    match peer {
        tl::enums::Peer::User(peer) => Some(peer.user_id),
        tl::enums::Peer::Chat(peer) => Some(peer.chat_id),
        tl::enums::Peer::Channel(peer) => Some(peer.channel_id),
    }
}

fn now_unix_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn now_unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(mute_until: Option<i32>) -> tl::enums::PeerNotifySettings {
        settings_with(None, mute_until)
    }

    fn settings_with(
        silent: Option<bool>,
        mute_until: Option<i32>,
    ) -> tl::enums::PeerNotifySettings {
        tl::types::PeerNotifySettings {
            show_previews: None,
            silent,
            mute_until,
            ios_sound: None,
            android_sound: None,
            other_sound: None,
            stories_muted: None,
            stories_hide_sender: None,
            stories_ios_sound: None,
            stories_android_sound: None,
            stories_other_sound: None,
        }
        .into()
    }

    #[test]
    fn future_mute_until_counts_as_muted() {
        let now = 1_000;

        assert!(peer_notify_settings_muted_at(&settings(Some(1_001)), now));
        assert!(!peer_notify_settings_muted_at(&settings(Some(999)), now));
        assert!(!peer_notify_settings_muted_at(&settings(Some(0)), now));
        assert!(!peer_notify_settings_muted_at(&settings(None), now));
    }

    #[test]
    fn silent_peer_settings_count_as_muted() {
        let now = 1_000;

        assert!(peer_notify_settings_muted_at(
            &settings_with(Some(true), None),
            now
        ));
        assert!(!peer_notify_settings_muted_at(
            &settings_with(Some(false), None),
            now
        ));
    }

    #[test]
    fn notify_peer_extracts_concrete_chat_ids() {
        let user = tl::types::NotifyPeer {
            peer: tl::types::PeerUser { user_id: 42 }.into(),
        };
        let group = tl::types::NotifyPeer {
            peer: tl::types::PeerChat { chat_id: 43 }.into(),
        };
        let channel = tl::types::NotifyPeer {
            peer: tl::types::PeerChannel { channel_id: 44 }.into(),
        };

        assert_eq!(notify_peer_chat_id(&user.into()), Some(42));
        assert_eq!(notify_peer_chat_id(&group.into()), Some(43));
        assert_eq!(notify_peer_chat_id(&channel.into()), Some(44));
        assert_eq!(
            notify_peer_chat_id(&tl::enums::NotifyPeer::NotifyBroadcasts),
            None
        );
    }

    #[test]
    fn notify_settings_updates_refresh_cache() {
        let mut cache = NotificationMuteCache::default();
        let peer: tl::enums::NotifyPeer = tl::types::NotifyPeer {
            peer: tl::types::PeerUser { user_id: 42 }.into(),
        }
        .into();

        let muted_update = tl::types::UpdateNotifySettings {
            peer: peer.clone(),
            notify_settings: settings(Some(i32::MAX)),
        }
        .into();
        cache.apply_raw_update(&muted_update);

        assert!(cache.muted_chat_ids.contains(&42));

        let unmuted_update = tl::types::UpdateNotifySettings {
            peer,
            notify_settings: settings(Some(0)),
        }
        .into();
        cache.apply_raw_update(&unmuted_update);

        assert!(!cache.muted_chat_ids.contains(&42));
    }
}
