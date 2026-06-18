//! JS snippets for the Lark/Feishu browser connector.

/// Returns logged-in status for the Lark/Feishu web app. Logged in once the
/// messenger shell is present and we're no longer on the accounts/login page.
/// Uses only stable hooks (no hashed classes).
pub(crate) const LARK_LOGIN_MARKER_JS: &str = r#"(() => {
  const onLogin = /accounts\.(larksuite|feishu)\.(com|cn)\/.*login/i.test(location.href);
  const shell = !!document.querySelector('.lark_feedMainList, .a11y_feed_main_list, [class*="page-content-messenger"]');
  return JSON.stringify({ loggedIn: shell && !onLogin, href: location.href });
})()"#;

/// Reads every conversation feed card (stable `[data-feed-id]` hook). Returns
/// chat_id, display name, last-message preview, unread flag, and a best-effort
/// outgoing flag (preview begins with the localized "You:" sender, detectable
/// via the self-message marker in the card or a leading sender span).
/// Also returns `loaded: bool` — true when the messenger shell is present in the
/// DOM (same stable selectors as the login marker), so callers can distinguish
/// "page not yet loaded / logged out" from "loaded but 0 conversations".
pub(crate) const LARK_FEED_SCRIPT: &str = r#"(() => {
  const loaded = !!document.querySelector('.lark_feedMainList, .a11y_feed_main_list, [class*="page-content-messenger"]');
  const cards = Array.from(document.querySelectorAll('[data-feed-id]'));
  const rows = cards.map(c => {
    const chat_id = c.getAttribute('data-feed-id') || '';
    const txt = (sel) => { const e = c.querySelector(sel); return e ? (e.textContent || '').trim() : ''; };
    // name/preview live under hashed classes; read by structural role via a11y where possible,
    // else fall back to the card's text lines. Keep selectors resilient: prefer [class*="a11y" i].
    const name = txt('[class*="a11y" i][class*="name" i]') || txt('[aria-label]') || '';
    const preview = (c.textContent || '').replace(name, '').trim().slice(0, 200);
    const unread = !!c.querySelector('[class*="badge" i]');
    const outgoing = /^you[:：]/i.test(preview);
    return { chat_id, name, preview, unread, outgoing };
  }).filter(r => r.chat_id);
  return JSON.stringify({ loaded, rows });
})()"#;

/// Returns `true` when the feed script result indicates the messenger shell was
/// present in the DOM at the time of the poll. Used to gate first-poll
/// initialization so an unloaded/logged-out page doesn't permanently seed an
/// empty baseline.
pub(crate) fn feed_loaded(result: &serde_json::Value) -> bool {
    result.get("loaded").and_then(|v| v.as_bool()).unwrap_or(false)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FeedRow {
    pub chat_id: String,
    pub name: String,
    pub preview: String,
    pub unread: bool,
    pub is_outgoing: bool,
}

pub(crate) fn parse_feed_rows(result: &serde_json::Value) -> Vec<FeedRow> {
    result.get("rows").and_then(|v| v.as_array()).map(|rows| {
        rows.iter().filter_map(|r| {
            let chat_id = r.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").trim().to_string();
            if chat_id.is_empty() { return None; }
            Some(FeedRow {
                chat_id,
                name: r.get("name").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                preview: r.get("preview").and_then(|v| v.as_str()).unwrap_or("").to_string(),
                unread: r.get("unread").and_then(|v| v.as_bool()).unwrap_or(false),
                is_outgoing: r.get("outgoing").and_then(|v| v.as_bool()).unwrap_or(false),
            })
        }).collect()
    }).unwrap_or_default()
}

#[cfg(test)]
mod feed_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn feed_loaded_true_when_loaded_key_is_true() {
        let result = json!({"loaded": true, "rows": []});
        assert!(feed_loaded(&result));
    }

    #[test]
    fn feed_loaded_false_when_loaded_key_is_false() {
        let result = json!({"loaded": false, "rows": []});
        assert!(!feed_loaded(&result));
    }

    #[test]
    fn feed_loaded_false_when_loaded_key_missing() {
        // Simulates an unloaded page or old script version missing the key.
        let result = json!({"rows": []});
        assert!(!feed_loaded(&result));
    }

    #[test]
    fn feed_loaded_false_on_null() {
        assert!(!feed_loaded(&serde_json::Value::Null));
    }

    #[test]
    fn parses_feed_rows_with_chat_id_and_direction() {
        let result = json!({"rows": [
            {"chat_id": "7651002084879241330", "name": "Alice", "preview": "hi there", "unread": true, "outgoing": false},
            {"chat_id": "7650335261468921967", "name": "Bob", "preview": "You: on it", "unread": false, "outgoing": true}
        ]});
        let rows = parse_feed_rows(&result);
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].chat_id, "7651002084879241330");
        assert!(!rows[0].is_outgoing);
        assert!(rows[0].unread);
        assert_eq!(rows[1].chat_id, "7650335261468921967");
        assert!(rows[1].is_outgoing);
    }

    #[test]
    fn skips_rows_without_chat_id() {
        let result = json!({"rows": [{"name": "x", "preview": "y"}]});
        assert!(parse_feed_rows(&result).is_empty());
    }
}
