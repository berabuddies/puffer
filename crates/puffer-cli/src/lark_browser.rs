//! Lark/Feishu web connector backed by daemon-managed CEF sessions.

use puffer_subscriber_runtime::Event;
use serde_json::json;
use std::collections::BTreeSet;

pub(crate) const CONNECTOR_SLUG_LARK: &str = "lark-browser";
pub(crate) const CONNECTOR_SLUG_FEISHU: &str = "feishu-browser";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Brand {
    Lark,
    Feishu,
}

impl Brand {
    pub(crate) fn from_slug(slug: &str) -> Option<Brand> {
        match slug {
            CONNECTOR_SLUG_LARK => Some(Brand::Lark),
            CONNECTOR_SLUG_FEISHU => Some(Brand::Feishu),
            _ => None,
        }
    }
    pub(crate) fn slug(&self) -> &'static str {
        match self {
            Brand::Lark => CONNECTOR_SLUG_LARK,
            Brand::Feishu => CONNECTOR_SLUG_FEISHU,
        }
    }
    pub(crate) fn platform(&self) -> &'static str {
        self.slug()
    }
    pub(crate) fn web_url(&self) -> &'static str {
        match self {
            Brand::Lark => "https://web.larksuite.com/",
            Brand::Feishu => "https://web.feishu.cn/",
        }
    }
}

/// Persisted Lark/Feishu browser connector configuration.
#[allow(dead_code)] // wired into run_subscriber in Task 7
#[derive(Clone, Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct LarkBrowserConfig {
    #[serde(default)]
    pub(crate) brand: String,
    #[serde(default)]
    pub(crate) connection: String,
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
struct SeenState {
    #[serde(default)]
    initialized: bool,
    #[serde(default)]
    seen: BTreeSet<String>,
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
fn feed_fingerprint(preview: &str) -> String {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    preview.trim().hash(&mut h);
    format!("{:x}", h.finish())
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
fn feed_dedup_key(conn: &str, row: &crate::lark_browser_script::FeedRow) -> String {
    format!("{}:{}:{}", conn, row.chat_id, feed_fingerprint(&row.preview))
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
fn should_emit_feed(seen: &SeenState, key: &str) -> bool {
    if seen.seen.contains(key) {
        return false;
    }
    seen.initialized // pre-init: seeds only, emits nothing
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
fn build_message_event(
    platform: &str,
    brand: &str,
    chat_id: &str,
    sender: &str,
    text: &str,
    is_outgoing: bool,
    source: &str,
    dedup_key: &str,
) -> Event {
    Event {
        topic: platform.to_string(),
        kind: "message".to_string(),
        control: false,
        dedup_key: Some(dedup_key.to_string()),
        text: format!("{sender}\n{text}").trim().to_string(),
        payload: json!({
            "platform": platform,
            "brand": brand,
            "chat_id": chat_id,
            "sender": sender,
            "is_outgoing": is_outgoing,
            "source": source,
            "receivedAtMs": now_ms(),
        }),
    }
}

#[allow(dead_code)] // wired into run_subscriber in Task 7
fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
}

pub(crate) async fn run_subscriber() -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod emit_tests {
    use super::*;
    use crate::lark_browser_script::FeedRow;

    fn row(chat: &str, preview: &str, out: bool) -> FeedRow {
        FeedRow { chat_id: chat.into(), name: "N".into(), preview: preview.into(), unread: true, is_outgoing: out }
    }

    #[test]
    fn first_poll_seeds_without_emitting() {
        let mut seen = SeenState::default();
        let key = feed_dedup_key("c1", &row("123", "hi", false));
        assert!(!should_emit_feed(&seen, &key));      // pre-init: do not emit
        seen.seen.insert(key.clone());
        seen.initialized = true;
        let key2 = feed_dedup_key("c1", &row("123", "new msg", false));
        assert!(should_emit_feed(&seen, &key2));      // post-init: emit new
        assert!(!should_emit_feed(&seen, &key));       // already seen: skip
    }

    #[test]
    fn event_payload_has_monitor_keys() {
        let ev = build_message_event("lark-browser", "lark", "123", "Alice", "hi", true, "feed", "c1:123:abc");
        assert_eq!(ev.payload["chat_id"], "123");
        assert_eq!(ev.payload["is_outgoing"], true);
        assert_eq!(ev.payload["platform"], "lark-browser");
        assert_eq!(ev.payload["brand"], "lark");
        assert_eq!(ev.kind, "message");
        assert_eq!(ev.dedup_key.as_deref(), Some("c1:123:abc"));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn brand_from_slug_maps_both_brands() {
        assert_eq!(Brand::from_slug("lark-browser"), Some(Brand::Lark));
        assert_eq!(Brand::from_slug("feishu-browser"), Some(Brand::Feishu));
        assert_eq!(Brand::from_slug("gmail-browser"), None);
    }

    #[test]
    fn brand_web_url_and_platform() {
        assert_eq!(Brand::Lark.web_url(), "https://web.larksuite.com/");
        assert_eq!(Brand::Feishu.web_url(), "https://web.feishu.cn/");
        assert_eq!(Brand::Lark.platform(), "lark-browser");
        assert_eq!(Brand::Feishu.platform(), "feishu-browser");
    }
}
