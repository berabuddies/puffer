//! Shared contact identity helpers for connector events.
//!
//! Contact ids are stable, user-visible routing keys. The prefix before the
//! first `@` selects one connector family; the suffix is the connector-owned
//! identity for a person on that service.

use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeSet;

/// Contact id prefix for Telegram user accounts.
pub const TELEGRAM_CONTACT_PREFIX: &str = "telegram";
/// Contact id prefix for Telegram numeric user ids.
pub const TELEGRAM_USER_ID_CONTACT_PREFIX: &str = "telegram-user-id";
/// Contact id prefix for Telegram numeric chat ids.
pub const TELEGRAM_CHAT_ID_CONTACT_PREFIX: &str = "telegram-chat-id";
/// Contact id prefix shared by Gmail, Google Calendar, and generic email.
pub const GOOGLE_CONTACT_PREFIX: &str = "google";
/// Contact id prefix for Slack users.
pub const SLACK_CONTACT_PREFIX: &str = "slack";
/// Contact id prefix for Discord users.
pub const DISCORD_CONTACT_PREFIX: &str = "discord";
/// Contact id prefix for Matrix users.
pub const MATRIX_CONTACT_PREFIX: &str = "matrix";
/// Contact id prefix for Lark users.
pub const LARK_CONTACT_PREFIX: &str = "lark";

/// One normalized connector contact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ConnectorContact {
    /// Stable contact id, for example `telegram@alice` or
    /// `google@alice@example.com`.
    pub id: String,
    /// Optional avatar URL or data URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Display name shown in contact pickers.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// Connector context snippets associated with this contact.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub context: Vec<ContactContext>,
    /// Higher scores are more reactive/important.
    #[serde(default)]
    pub score: f64,
}

/// One context item attached to a contact.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq)]
pub struct ContactContext {
    /// Connector-specific message or event kind.
    pub kind: String,
    /// Human-readable context text.
    pub text: String,
    /// Optional event timestamp in milliseconds since UNIX epoch.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timestamp_ms: Option<i128>,
    /// Raw connector payload for callers that need typed fields.
    #[serde(default)]
    pub payload: Value,
}

/// A user-curated contact that can group multiple connector contact ids.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct SavedContact {
    /// Stable local contact id.
    pub id: String,
    /// User-authored label.
    pub name: String,
    /// User-authored description used by monitor triage.
    pub description: String,
    /// Optional avatar URL or data URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Connector contact ids grouped under this saved contact.
    #[serde(default)]
    pub contact_ids: Vec<String>,
}

/// LLM-generated contact proposal.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
pub struct ContactProposal {
    /// Proposed display name.
    pub name: String,
    /// Two-sentence explanation of why this is important and not spam.
    pub description: String,
    /// Optional avatar URL or data URI.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avatar: Option<String>,
    /// Connector contact ids the proposal groups together.
    #[serde(default)]
    pub contact_ids: Vec<String>,
}

/// Returns the lowercase prefix of a valid contact id.
pub fn contact_id_prefix(contact_id: &str) -> Option<&str> {
    let (prefix, suffix) = contact_id.split_once('@')?;
    if prefix.is_empty() || suffix.is_empty() {
        return None;
    }
    Some(prefix)
}

/// Normalizes a user-entered contact id.
pub fn normalize_contact_id(input: &str) -> Option<String> {
    let trimmed = input.trim();
    let (prefix, suffix) = trimmed.split_once('@')?;
    let prefix = prefix.trim().to_ascii_lowercase();
    let suffix = normalize_contact_suffix(&prefix, suffix)?;
    match prefix.as_str() {
        TELEGRAM_CONTACT_PREFIX
        | TELEGRAM_USER_ID_CONTACT_PREFIX
        | TELEGRAM_CHAT_ID_CONTACT_PREFIX
        | GOOGLE_CONTACT_PREFIX
        | SLACK_CONTACT_PREFIX
        | DISCORD_CONTACT_PREFIX
        | MATRIX_CONTACT_PREFIX
        | LARK_CONTACT_PREFIX => Some(format!("{prefix}@{suffix}")),
        _ => None,
    }
}

/// Returns connector slugs that may own `contact_id`.
pub fn connector_slugs_for_contact_id(contact_id: &str) -> Vec<&'static str> {
    match contact_id_prefix(contact_id) {
        Some(TELEGRAM_CONTACT_PREFIX)
        | Some(TELEGRAM_USER_ID_CONTACT_PREFIX)
        | Some(TELEGRAM_CHAT_ID_CONTACT_PREFIX) => vec!["telegram-login", "telegram-bot"],
        Some(GOOGLE_CONTACT_PREFIX) => vec!["email", "gmail-browser", "gcal-browser"],
        Some(SLACK_CONTACT_PREFIX) => vec!["slack-login", "slack-app", "slack-bot"],
        Some(DISCORD_CONTACT_PREFIX) => vec!["discord-bot"],
        Some(MATRIX_CONTACT_PREFIX) => vec!["matrix-bot"],
        Some(LARK_CONTACT_PREFIX) => vec!["lark-login", "lark-bot"],
        _ => Vec::new(),
    }
}

/// Returns true when a connector slug may own the contact id.
pub fn connector_slug_accepts_contact_id(connector_slug: &str, contact_id: &str) -> bool {
    connector_slugs_for_contact_id(contact_id)
        .into_iter()
        .any(|slug| slug == connector_slug)
}

/// Normalizes and filters contact ids to the family owned by one connector.
pub fn contact_ids_for_connector(connector_slug: &str, ids: &[String]) -> Vec<String> {
    ids.iter()
        .filter_map(|id| normalize_contact_id(id))
        .filter(|id| connector_slug_accepts_contact_id(connector_slug, id))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

/// Drops contact rows whose ids do not belong to the connector family.
pub fn connector_contacts_for_connector(
    connector_slug: &str,
    contacts: Vec<ConnectorContact>,
) -> Vec<ConnectorContact> {
    contacts
        .into_iter()
        .filter_map(|mut contact| {
            let id = normalize_contact_id(&contact.id)?;
            if !connector_slug_accepts_contact_id(connector_slug, &id) {
                return None;
            }
            contact.id = id;
            Some(contact)
        })
        .collect()
}

/// Extracts every normalized contact id implied by a connector event payload.
pub fn contact_ids_from_payload(payload: &Value) -> Vec<String> {
    let mut ids = BTreeSet::new();
    collect_explicit_ids(payload, &mut ids);
    collect_telegram_ids(payload, &mut ids);
    collect_google_ids(payload, &mut ids);
    collect_prefixed_ids(
        payload,
        SLACK_CONTACT_PREFIX,
        &mut ids,
        &["slack_user", "slack_user_id"],
    );
    collect_prefixed_ids(
        payload,
        DISCORD_CONTACT_PREFIX,
        &mut ids,
        &["discord_user", "discord_user_id"],
    );
    collect_prefixed_ids(
        payload,
        MATRIX_CONTACT_PREFIX,
        &mut ids,
        &["matrix_user", "matrix_user_id"],
    );
    collect_prefixed_ids(
        payload,
        LARK_CONTACT_PREFIX,
        &mut ids,
        &["lark_user", "lark_user_id", "sender_open_id"],
    );
    collect_hinted_prefixed_ids(
        payload,
        SLACK_CONTACT_PREFIX,
        &mut ids,
        &["author_id", "user_id"],
    );
    collect_hinted_prefixed_ids(
        payload,
        DISCORD_CONTACT_PREFIX,
        &mut ids,
        &["author_id", "user_id"],
    );
    collect_hinted_prefixed_ids(
        payload,
        MATRIX_CONTACT_PREFIX,
        &mut ids,
        &["sender", "sender_id"],
    );
    collect_hinted_prefixed_ids(payload, LARK_CONTACT_PREFIX, &mut ids, &["sender_id"]);
    ids.into_iter().collect()
}

/// Returns true when `filter_ids` is empty or intersects payload contact ids.
pub fn contact_filter_matches(filter_ids: &[String], payload: &Value) -> bool {
    if filter_ids.is_empty() {
        return true;
    }
    let payload_ids = contact_ids_from_payload(payload)
        .into_iter()
        .collect::<BTreeSet<_>>();
    filter_ids.iter().any(|id| {
        normalize_contact_id(id)
            .as_ref()
            .is_some_and(|normalized| payload_ids.contains(normalized))
    })
}

/// Normalizes, deduplicates, and sorts a contact id list.
pub fn normalize_contact_ids<I, S>(ids: I) -> Vec<String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<str>,
{
    ids.into_iter()
        .filter_map(|id| normalize_contact_id(id.as_ref()))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}

fn normalize_contact_suffix(prefix: &str, suffix: &str) -> Option<String> {
    let mut suffix = suffix.trim().trim_start_matches('@').to_string();
    if suffix.is_empty() {
        return None;
    }
    match prefix {
        TELEGRAM_CONTACT_PREFIX => {
            suffix = suffix.to_ascii_lowercase();
            if suffix.chars().all(|ch| ch.is_ascii_digit())
                || !suffix
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            {
                return None;
            }
        }
        TELEGRAM_USER_ID_CONTACT_PREFIX | TELEGRAM_CHAT_ID_CONTACT_PREFIX => {
            let digits = suffix.strip_prefix('-').unwrap_or(&suffix);
            if digits.is_empty() || !digits.chars().all(|ch| ch.is_ascii_digit()) {
                return None;
            }
        }
        _ => {}
    }
    if prefix == GOOGLE_CONTACT_PREFIX {
        suffix = suffix.to_ascii_lowercase();
    }
    if suffix
        .chars()
        .any(|ch| ch.is_control() || ch.is_whitespace())
    {
        return None;
    }
    Some(suffix)
}

fn collect_explicit_ids(payload: &Value, ids: &mut BTreeSet<String>) {
    if let Some(id) = payload.get("contact_id").and_then(Value::as_str) {
        insert_id(ids, id);
    }
    if let Some(values) = payload.get("contact_ids").and_then(Value::as_array) {
        for value in values {
            if let Some(id) = value.as_str() {
                insert_id(ids, id);
            }
        }
    }
}

fn collect_telegram_ids(payload: &Value, ids: &mut BTreeSet<String>) {
    let chat_kind = string_at(payload, &["chat_kind"]);
    let direct_user = chat_kind.as_deref() == Some("user");
    if direct_user {
        if payload.get("chat_is_bot").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(username) = string_at(payload, &["chat_username"]) {
            insert_prefixed(ids, TELEGRAM_CONTACT_PREFIX, &username);
        } else if let Some(chat_id) = payload.get("chat_id").and_then(Value::as_i64) {
            insert_prefixed(ids, TELEGRAM_USER_ID_CONTACT_PREFIX, &chat_id.to_string());
        }
    }
    if chat_kind.as_deref() == Some("group") {
        if let Some(chat_id) = payload.get("chat_id").and_then(Value::as_i64) {
            insert_prefixed(ids, TELEGRAM_CHAT_ID_CONTACT_PREFIX, &chat_id.to_string());
        }
    }
    if !direct_user || string_at(payload, &["sender_username"]).is_some() {
        if payload.get("sender_is_bot").and_then(Value::as_bool) == Some(true) {
            return;
        }
        if let Some(username) = string_at(payload, &["sender_username"]) {
            insert_prefixed(ids, TELEGRAM_CONTACT_PREFIX, &username);
        } else if let Some(sender_id) = payload.get("sender_id").and_then(Value::as_i64) {
            insert_prefixed(ids, TELEGRAM_USER_ID_CONTACT_PREFIX, &sender_id.to_string());
        }
    }
}

fn collect_google_ids(payload: &Value, ids: &mut BTreeSet<String>) {
    for path in [
        &["from"][..],
        &["from_email"],
        &["sender_email"],
        &["organizer_email"],
        &["account"],
        &["message", "sender"],
        &["message", "from"],
        &["event", "organizer"],
        &["event", "summary"],
        &["event", "title"],
    ] {
        if let Some(value) = string_at(payload, path) {
            for email in extract_emails(&value) {
                insert_prefixed(ids, GOOGLE_CONTACT_PREFIX, &email);
            }
        }
    }
    for path in [&["to"][..], &["cc"], &["attendees"], &["message", "to"]] {
        if let Some(value) = payload.pointer(&json_pointer(path)) {
            collect_google_ids_from_value(value, ids);
        }
    }
}

fn collect_google_ids_from_value(value: &Value, ids: &mut BTreeSet<String>) {
    match value {
        Value::String(value) => {
            for email in extract_emails(value) {
                insert_prefixed(ids, GOOGLE_CONTACT_PREFIX, &email);
            }
        }
        Value::Array(values) => {
            for value in values {
                collect_google_ids_from_value(value, ids);
            }
        }
        Value::Object(map) => {
            for key in ["email", "address", "mail"] {
                if let Some(value) = map.get(key) {
                    collect_google_ids_from_value(value, ids);
                }
            }
        }
        _ => {}
    }
}

fn collect_prefixed_ids(payload: &Value, prefix: &str, ids: &mut BTreeSet<String>, keys: &[&str]) {
    for key in keys {
        if let Some(value) = scalar_string_at(payload, &[*key]) {
            insert_prefixed(ids, prefix, &value);
        }
    }
}

fn collect_hinted_prefixed_ids(
    payload: &Value,
    prefix: &str,
    ids: &mut BTreeSet<String>,
    keys: &[&str],
) {
    if !payload_mentions_prefix(payload, prefix) {
        return;
    }
    collect_prefixed_ids(payload, prefix, ids, keys);
}

fn payload_mentions_prefix(payload: &Value, prefix: &str) -> bool {
    for key in [
        "connector",
        "connector_slug",
        "source",
        "source_connector",
        "service",
        "platform",
    ] {
        if string_at(payload, &[key])
            .as_deref()
            .is_some_and(|value| value.to_ascii_lowercase().contains(prefix))
        {
            return true;
        }
    }
    false
}

fn insert_id(ids: &mut BTreeSet<String>, value: &str) {
    if let Some(id) = normalize_contact_id(value) {
        ids.insert(id);
    }
}

fn insert_prefixed(ids: &mut BTreeSet<String>, prefix: &str, suffix: &str) {
    insert_id(ids, &format!("{prefix}@{suffix}"));
}

fn string_at(payload: &Value, path: &[&str]) -> Option<String> {
    payload
        .pointer(&json_pointer(path))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn scalar_string_at(payload: &Value, path: &[&str]) -> Option<String> {
    let value = payload.pointer(&json_pointer(path))?;
    match value {
        Value::String(value) => Some(value.trim().to_string()).filter(|value| !value.is_empty()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn json_pointer(path: &[&str]) -> String {
    let mut out = String::new();
    for item in path {
        out.push('/');
        out.push_str(&item.replace('~', "~0").replace('/', "~1"));
    }
    out
}

fn extract_emails(value: &str) -> Vec<String> {
    value
        .split(|ch: char| {
            !(ch.is_ascii_alphanumeric() || matches!(ch, '@' | '.' | '_' | '+' | '-'))
        })
        .filter_map(|part| {
            let trimmed =
                part.trim_matches(|ch: char| matches!(ch, '<' | '>' | '"' | '\'' | ',' | ';'));
            looks_like_email(trimmed).then(|| trimmed.to_ascii_lowercase())
        })
        .collect()
}

fn looks_like_email(value: &str) -> bool {
    let Some((local, domain)) = value.split_once('@') else {
        return false;
    };
    !local.is_empty() && domain.contains('.') && !domain.starts_with('.') && !domain.ends_with('.')
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn normalizes_known_contact_ids() {
        assert_eq!(
            normalize_contact_id(" Telegram@@ALICE "),
            Some("telegram@alice".to_string())
        );
        assert_eq!(normalize_contact_id("telegram@12345"), None);
        assert_eq!(
            normalize_contact_id(" telegram-user-id@5229190700 "),
            Some("telegram-user-id@5229190700".to_string())
        );
        assert_eq!(
            normalize_contact_id(" telegram-chat-id@-100123 "),
            Some("telegram-chat-id@-100123".to_string())
        );
        assert_eq!(
            normalize_contact_id("google@Alice@Example.COM"),
            Some("google@alice@example.com".to_string())
        );
        assert_eq!(normalize_contact_id("unknown@x"), None);
    }

    #[test]
    fn extracts_telegram_direct_and_group_contacts() {
        assert_eq!(
            contact_ids_from_payload(&json!({
                "chat_kind": "user",
                "chat_username": "alice",
                "sender_username": "alice"
            })),
            vec!["telegram@alice"]
        );
        assert_eq!(
            contact_ids_from_payload(&json!({
                "chat_kind": "group",
                "chat_id": -1,
                "sender_username": "bob"
            })),
            vec!["telegram-chat-id@-1", "telegram@bob"]
        );
        assert_eq!(
            contact_ids_from_payload(&json!({
                "chat_kind": "group",
                "chat_id": -1,
                "sender_id": 42
            })),
            vec!["telegram-chat-id@-1", "telegram-user-id@42"]
        );
        assert_eq!(
            contact_ids_from_payload(&json!({
                "chat_kind": "user",
                "chat_id": 5229190700_i64,
                "sender_id": 5229190700_i64
            })),
            vec!["telegram-user-id@5229190700"]
        );
        assert!(contact_ids_from_payload(&json!({
            "chat_kind": "user",
            "chat_username": "alertbot",
            "chat_is_bot": true
        }))
        .is_empty());
    }

    #[test]
    fn extracts_google_contacts_from_email_payloads() {
        let ids = contact_ids_from_payload(&json!({
            "from": "Alice <Alice@Example.COM>",
            "to": ["bob@example.com"],
            "message": {"sender": "Service <robot@example.net>"}
        }));

        assert_eq!(
            ids,
            vec![
                "google@alice@example.com",
                "google@bob@example.com",
                "google@robot@example.net"
            ]
        );
    }

    #[test]
    fn contact_filter_uses_normalized_payload_ids() {
        let payload = json!({"from": "Alice <alice@example.com>"});

        assert!(contact_filter_matches(
            &["google@ALICE@example.com".to_string()],
            &payload
        ));
        assert!(!contact_filter_matches(
            &["google@bob@example.com".to_string()],
            &payload
        ));
        assert!(contact_filter_matches(
            &["telegram-user-id@42".to_string()],
            &json!({"chat_kind":"group","chat_id":-1,"sender_id":42})
        ));
    }

    #[test]
    fn connector_family_filter_keeps_only_owned_contact_ids() {
        assert_eq!(
            contact_ids_for_connector(
                "telegram-login",
                &[
                    "google@alice@example.com".to_string(),
                    "telegram@ALICE".to_string(),
                    "telegram-user-id@42".to_string(),
                    "telegram-chat-id@-1".to_string(),
                    "lark@ou_1".to_string(),
                ],
            ),
            vec![
                "telegram-chat-id@-1",
                "telegram-user-id@42",
                "telegram@alice"
            ]
        );
        let contacts = connector_contacts_for_connector(
            "telegram-login",
            vec![
                ConnectorContact {
                    id: "Telegram@ALICE".into(),
                    avatar: None,
                    name: None,
                    context: Vec::new(),
                    score: 1.0,
                },
                ConnectorContact {
                    id: "google@alice@example.com".into(),
                    avatar: None,
                    name: None,
                    context: Vec::new(),
                    score: 1.0,
                },
            ],
        );
        assert_eq!(contacts.len(), 1);
        assert_eq!(contacts[0].id, "telegram@alice");
    }
}
