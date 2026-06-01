//! Telegram notification mute helpers.

use std::collections::BTreeSet;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::Context as _;
use grammers_client::{
    types::{Chat, Dialog, Message},
    Client,
};
use grammers_tl_types as tl;

/// Returns whether a dialog's current peer notification settings suppress messages.
pub(crate) fn dialog_notifications_suppressed(dialog: &Dialog) -> bool {
    dialog_notifications_suppressed_at(dialog, now_unix_seconds())
}

/// Returns whether the dialog is currently archived into a non-main folder.
pub(crate) fn dialog_archived(dialog: &Dialog) -> bool {
    dialog_raw_archived(&dialog.raw)
}

/// Returns whether this dialog should be suppressed from monitor delivery.
pub(crate) fn dialog_delivery_suppressed(dialog: &Dialog) -> bool {
    dialog_notifications_suppressed(dialog) || dialog_archived(dialog)
}

/// Fetches current peer notification settings and returns whether they suppress messages.
pub(crate) async fn fetch_chat_notification_suppressed(
    client: &Client,
    chat: &Chat,
) -> anyhow::Result<bool> {
    fetch_notify_peer_suppressed(
        client,
        tl::types::InputNotifyPeer {
            peer: chat.pack().to_input_peer(),
        }
        .into(),
    )
    .await
}

async fn fetch_notify_peer_suppressed(
    client: &Client,
    peer: tl::enums::InputNotifyPeer,
) -> anyhow::Result<bool> {
    let request = tl::functions::account::GetNotifySettings { peer };
    let settings = client
        .invoke(&request)
        .await
        .context("get Telegram notification settings")?;
    Ok(peer_notify_settings_muted_at(&settings, now_unix_seconds()))
}

/// Tracks peer suppression state observed from dialogs and raw updates.
#[derive(Debug, Default)]
pub(crate) struct NotificationMuteCache {
    muted_chat_ids: BTreeSet<i64>,
    archived_chat_ids: BTreeSet<i64>,
}

impl NotificationMuteCache {
    /// Returns whether the message's chat is known to be suppressed from socket state.
    pub(crate) fn message_chat_muted(&self, message: &Message) -> bool {
        self.chat_suppressed(message.chat().id())
    }

    /// Records suppression state from a Telegram dialog snapshot.
    pub(crate) fn observe_dialog(&mut self, dialog: &Dialog) -> bool {
        let chat_id = dialog.chat().id();
        let muted = dialog_notifications_suppressed(dialog);
        let archived = dialog_archived(dialog);
        self.set_chat_muted(chat_id, muted);
        self.set_chat_archived(chat_id, archived);
        muted || archived
    }

    /// Applies raw Telegram updates that change mute or archive state.
    pub(crate) fn apply_raw_update(&mut self, update: &tl::enums::Update) {
        match update {
            tl::enums::Update::NotifySettings(update) => match &update.peer {
                tl::enums::NotifyPeer::NotifyUsers
                | tl::enums::NotifyPeer::NotifyChats
                | tl::enums::NotifyPeer::NotifyBroadcasts => {}
                _ => {
                    let Some(chat_id) = notify_peer_chat_id(&update.peer) else {
                        return;
                    };
                    let muted =
                        peer_notify_settings_muted_at(&update.notify_settings, now_unix_seconds());
                    self.set_chat_muted(chat_id, muted);
                }
            },
            tl::enums::Update::FolderPeers(update) => {
                for peer in &update.folder_peers {
                    let tl::enums::FolderPeer::Peer(peer) = peer;
                    if let Some(chat_id) = peer_chat_id(&peer.peer) {
                        self.set_chat_archived(chat_id, peer.folder_id != 0);
                    }
                }
            }
            _ => {}
        }
    }

    fn set_chat_muted(&mut self, chat_id: i64, muted: bool) {
        if muted {
            self.muted_chat_ids.insert(chat_id);
        } else {
            self.muted_chat_ids.remove(&chat_id);
        }
    }

    fn set_chat_archived(&mut self, chat_id: i64, archived: bool) {
        if archived {
            self.archived_chat_ids.insert(chat_id);
        } else {
            self.archived_chat_ids.remove(&chat_id);
        }
    }

    fn chat_suppressed(&self, chat_id: i64) -> bool {
        self.muted_chat_ids.contains(&chat_id) || self.archived_chat_ids.contains(&chat_id)
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

fn dialog_raw_archived(dialog: &tl::enums::Dialog) -> bool {
    match dialog {
        tl::enums::Dialog::Dialog(dialog) => dialog.folder_id.is_some_and(|id| id != 0),
        tl::enums::Dialog::Folder(dialog) => folder_id(&dialog.folder).is_some_and(|id| id != 0),
    }
}

fn folder_id(folder: &tl::enums::Folder) -> Option<i32> {
    match folder {
        tl::enums::Folder::Folder(folder) => Some(folder.id),
    }
}

fn peer_notify_settings_muted_at(
    settings: &tl::enums::PeerNotifySettings,
    now_seconds: i64,
) -> bool {
    match settings {
        tl::enums::PeerNotifySettings::Settings(settings) => settings
            .mute_until
            .is_some_and(|mute_until| i64::from(mute_until) > now_seconds),
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
    fn silent_peer_settings_do_not_count_as_muted() {
        let now = 1_000;

        assert!(!peer_notify_settings_muted_at(
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

    #[test]
    fn global_notify_settings_do_not_suppress_individual_chats() {
        let mut cache = NotificationMuteCache::default();

        let muted_update = tl::types::UpdateNotifySettings {
            peer: tl::enums::NotifyPeer::NotifyChats,
            notify_settings: settings(Some(i32::MAX)),
        }
        .into();
        cache.apply_raw_update(&muted_update);

        assert!(cache.muted_chat_ids.is_empty());
        assert!(cache.archived_chat_ids.is_empty());
    }

    #[test]
    fn archived_dialogs_are_delivery_suppressed() {
        let dialog = tl::types::Dialog {
            pinned: false,
            unread_mark: false,
            view_forum_as_messages: false,
            peer: tl::types::PeerChannel { channel_id: 42 }.into(),
            top_message: 1,
            read_inbox_max_id: 0,
            read_outbox_max_id: 0,
            unread_count: 0,
            unread_mentions_count: 0,
            unread_reactions_count: 0,
            notify_settings: settings(Some(0)),
            pts: None,
            draft: None,
            folder_id: Some(1),
            ttl_period: None,
        };

        assert!(dialog_raw_archived(&dialog.into()));
    }

    #[test]
    fn folder_peer_updates_refresh_archive_cache() {
        let mut cache = NotificationMuteCache::default();

        let archived_update: tl::enums::Update = tl::types::UpdateFolderPeers {
            folder_peers: vec![tl::types::FolderPeer {
                peer: tl::types::PeerChannel { channel_id: 42 }.into(),
                folder_id: 1,
            }
            .into()],
            pts: 1,
            pts_count: 1,
        }
        .into();
        cache.apply_raw_update(&archived_update);

        assert!(cache.chat_suppressed(42));

        let unarchived_update: tl::enums::Update = tl::types::UpdateFolderPeers {
            folder_peers: vec![tl::types::FolderPeer {
                peer: tl::types::PeerChannel { channel_id: 42 }.into(),
                folder_id: 0,
            }
            .into()],
            pts: 2,
            pts_count: 1,
        }
        .into();
        cache.apply_raw_update(&unarchived_update);

        assert!(!cache.chat_suppressed(42));
    }

    #[test]
    fn unmute_does_not_clear_archive_suppression() {
        let mut cache = NotificationMuteCache::default();
        cache.set_chat_archived(42, true);
        let peer: tl::enums::NotifyPeer = tl::types::NotifyPeer {
            peer: tl::types::PeerChannel { channel_id: 42 }.into(),
        }
        .into();
        let unmuted_update = tl::types::UpdateNotifySettings {
            peer,
            notify_settings: settings(Some(0)),
        }
        .into();

        cache.apply_raw_update(&unmuted_update);

        assert!(cache.chat_suppressed(42));
    }
}
