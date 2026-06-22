//! JS snippets for the Lark/Feishu browser connector.

/// Returns logged-in status for the Lark/Feishu web app. After login the web app
/// redirects to a PER-TENANT subdomain (e.g. `plarkush1ube0xnn.usttp.larksuite.com`)
/// and lands on the default app (Drive), NOT the messenger — so login must NOT be
/// gated on the messenger shell. Detect login by being on a tenant brand host, off
/// the known pre-login/entry/login hosts (`accounts.`/`www.`/`web.`/…) and login paths.
pub(crate) const LARK_LOGIN_MARKER_JS: &str = r#"(() => {
  const h = location.host, p = location.pathname;
  const preLogin = /^(accounts|www|web|passport|sso|open|o|id)\./i.test(h);
  const onBrand = /(larksuite\.com|feishu\.cn)$/i.test(h);
  const onLoginPath = /\/(login|sso|passport|accounts)/i.test(p);
  return JSON.stringify({ loggedIn: onBrand && !preLogin && !onLoginPath, href: location.href });
})()"#;

/// Reads every conversation feed card (stable `[data-feed-id]` hook). Returns
/// chat_id, display name, last-message preview, unread flag, and a best-effort
/// outgoing flag (preview begins with the localized "You:" sender, detectable
/// via the self-message marker in the card or a leading sender span).
/// Also returns `loaded: bool` — true when the messenger shell is present in the
/// DOM (same stable selectors as the login marker), so callers can distinguish
/// "page not yet loaded / logged out" from "loaded but 0 conversations".
pub(crate) const LARK_FEED_SCRIPT: &str = r#"(() => {
  // The connector opens the web root, which lands on the default app (Drive) after
  // login; the message feed only exists under /next/messenger/. Navigate there once
  // (using the live tenant origin); the next poll finds the feed. Report loaded:false
  // while navigating so first-poll init isn't seeded on a non-messenger page.
  if (!/\/next\/messenger/.test(location.pathname)) {
    try { location.assign(location.origin + '/next/messenger/'); } catch (e) {}
    return JSON.stringify({ loaded: false, rows: [], navigating: true });
  }
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

// ── Task 8: Observer JS ──────────────────────────────────────────────────────

/// Installs an idempotent MutationObserver on the open conversation. Records
/// each new `.js-message-item` (stable `id`/`data-id`) with its direction
/// (`message-self`=out / `message-not-self`=in) into `window.__cap`. Re-running
/// is a no-op once installed.
pub(crate) const LARK_OBSERVER_INSTALL_JS: &str = r#"(() => {
  window.__cap = window.__cap || [];
  if (window.__capObs) return JSON.stringify({status:'already'});
  const seen = new Set();
  const record = el => {
    if (!el || !el.matches || !el.matches('.js-message-item')) return;
    const id = el.id || el.getAttribute('data-id');
    if (!id || seen.has(id)) return;
    seen.add(id);
    const cls = (el.className || '').toString();
    const dir = cls.includes('message-not-self') ? 'in' : (cls.includes('message-self') ? 'out' : '?');
    const t = el.querySelector('.message-text');
    window.__cap.push({id, dir, pos: el.getAttribute('data-position'), text: t ? (t.textContent||'').trim().slice(0,2000) : ''});
  };
  document.querySelectorAll('.js-message-item').forEach(record);
  window.__capObs = new MutationObserver(muts => {
    for (const m of muts) for (const n of m.addedNodes) {
      if (n.nodeType !== 1) continue;
      record(n);
      if (n.querySelectorAll) n.querySelectorAll('.js-message-item').forEach(record);
    }
  });
  window.__capObs.observe(document.body, {childList:true, subtree:true});
  return JSON.stringify({status:'installed', seeded: window.__cap.length});
})()"#;

/// Returns and CLEARS window.__cap (drain). The active chat id is read from the
/// feed card marked `[data-feed-active="true"]`.
pub(crate) const LARK_OBSERVER_DRAIN_JS: &str = r#"(() => {
  const cap = window.__cap || [];
  window.__cap = [];
  const active = document.querySelector('[data-feed-active="true"]');
  const chat_id = active ? (active.getAttribute('data-feed-id') || '') : '';
  return JSON.stringify({ chat_id, items: cap });
})()"#;

// ── Task 9: Active-message parser + optimistic-id reconciliation ─────────────

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ActiveMsg {
    pub id: String,
    pub is_outgoing: bool,
    pub text: String,
}

pub(crate) fn is_snowflake_id(id: &str) -> bool {
    id.len() >= 15 && id.bytes().all(|b| b.is_ascii_digit())
}

pub(crate) fn parse_active_drain(result: &serde_json::Value) -> (String, Vec<ActiveMsg>) {
    let chat_id = result.get("chat_id").and_then(|v| v.as_str()).unwrap_or("").to_string();
    let msgs = result.get("items").and_then(|v| v.as_array()).map(|items| {
        items.iter().filter_map(|m| {
            let id = m.get("id").and_then(|v| v.as_str()).unwrap_or("");
            if !is_snowflake_id(id) { return None; } // drop pending optimistic ids
            Some(ActiveMsg {
                id: id.to_string(),
                is_outgoing: m.get("dir").and_then(|v| v.as_str()) == Some("out"),
                text: m.get("text").and_then(|v| v.as_str()).unwrap_or("").to_string(),
            })
        }).collect()
    }).unwrap_or_default();
    (chat_id, msgs)
}

#[cfg(test)]
mod active_tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn snowflake_detection_filters_optimistic_ids() {
        assert!(is_snowflake_id("7652607780750119026"));
        assert!(!is_snowflake_id("gApEI0EY3S"));   // optimistic temp id
        assert!(!is_snowflake_id("123"));
    }

    #[test]
    fn parse_drain_drops_pending_optimistic_messages() {
        let result = json!({"chat_id":"999","items":[
            {"id":"gApEI0EY3S","dir":"out","text":"sending"},
            {"id":"7652607780750119026","dir":"out","text":"sent"},
            {"id":"7652607883305029745","dir":"in","text":"reply"}
        ]});
        let (chat, msgs) = parse_active_drain(&result);
        assert_eq!(chat, "999");
        assert_eq!(msgs.len(), 2);                  // optimistic id dropped
        assert_eq!(msgs[0].id, "7652607780750119026");
        assert!(msgs[0].is_outgoing);
        assert!(!msgs[1].is_outgoing);
    }
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
