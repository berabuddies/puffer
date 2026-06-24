use anyhow::{bail, Context, Result};
use serde_json::{json, Map, Value};
use sha2::{Digest, Sha256};

pub const MONITOR_SCHEMA_VERSION: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MonitorTaskKind {
    TelegramReply,
    GmailReply,
    CalendarRsvp,
    GenericReview,
}

impl MonitorTaskKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::TelegramReply => "telegram.reply",
            Self::GmailReply => "gmail.reply",
            Self::CalendarRsvp => "calendar.rsvp",
            Self::GenericReview => "generic.review",
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MonitorContract {
    pub schema_version: u64,
    pub kind: MonitorTaskKind,
    pub source: Map<String, Value>,
    pub action: Map<String, Value>,
    pub source_hash: Option<String>,
}

pub fn parse_monitor_contract(metadata: &Map<String, Value>) -> Result<Option<MonitorContract>> {
    let Some(monitor) = metadata.get("monitor") else {
        return Ok(None);
    };
    let monitor = monitor
        .as_object()
        .context("monitor metadata must be an object")?;
    reject_conflicting_top_level_typed_blocks(metadata)?;

    let schema_version = monitor
        .get("schema_version")
        .or_else(|| monitor.get("schemaVersion"))
        .and_then(Value::as_u64)
        .context("monitor.schema_version must be 2")?;
    if schema_version != MONITOR_SCHEMA_VERSION {
        bail!("unsupported monitor.schema_version {schema_version}");
    }

    let kind = parse_monitor_kind(
        monitor
            .get("kind")
            .and_then(Value::as_str)
            .context("monitor.kind is required")?,
    )?;
    let source = required_object(monitor, "source")?.clone();
    let action = required_object(monitor, "action")?.clone();
    Ok(Some(MonitorContract {
        schema_version,
        kind,
        source,
        action,
        source_hash: string_field(monitor, &["source_hash", "sourceHash"]),
    }))
}

pub fn display_source_context(contract: &MonitorContract) -> Value {
    match contract.kind {
        MonitorTaskKind::TelegramReply => telegram_source_context(contract),
        MonitorTaskKind::GmailReply => gmail_source_context(contract),
        MonitorTaskKind::CalendarRsvp => calendar_source_context(contract),
        MonitorTaskKind::GenericReview => generic_source_context(contract),
    }
}

pub fn monitor_contract_hash(contract: &MonitorContract) -> Result<String> {
    let canonical = json!({
        "schema_version": contract.schema_version,
        "kind": contract.kind.as_str(),
        "source": contract.source,
        "action": contract.action,
    });
    let raw = canonical_json(&canonical)?;
    Ok(format!("sha256:{:x}", Sha256::digest(raw.as_bytes())))
}

fn parse_monitor_kind(value: &str) -> Result<MonitorTaskKind> {
    match value {
        "telegram.reply" => Ok(MonitorTaskKind::TelegramReply),
        "gmail.reply" => Ok(MonitorTaskKind::GmailReply),
        "calendar.rsvp" => Ok(MonitorTaskKind::CalendarRsvp),
        "generic.review" => Ok(MonitorTaskKind::GenericReview),
        _ => bail!("unsupported monitor.kind `{value}`"),
    }
}

fn reject_conflicting_top_level_typed_blocks(metadata: &Map<String, Value>) -> Result<()> {
    for key in ["telegram", "gmail", "calendar", "google_calendar"] {
        if metadata.contains_key(key) {
            bail!("conflicting typed monitor metadata `{key}` cannot appear beside monitor");
        }
    }
    Ok(())
}

fn required_object<'a>(
    object: &'a Map<String, Value>,
    key: &str,
) -> Result<&'a Map<String, Value>> {
    object
        .get(key)
        .and_then(Value::as_object)
        .with_context(|| format!("monitor.{key} must be an object"))
}

fn telegram_source_context(contract: &MonitorContract) -> Value {
    let chat_id = string_field(&contract.source, &["chat_id", "chatId"]).unwrap_or_default();
    let chat_kind = string_field(&contract.source, &["chat_kind", "chatKind"])
        .unwrap_or_else(|| "user".to_string());
    let (kind, summary_label) = match chat_kind.as_str() {
        "group" | "supergroup" => ("telegram_group_message", "Telegram group message"),
        "channel" => ("telegram_channel_message", "Telegram channel message"),
        _ => ("telegram_direct_message", "Telegram direct message"),
    };
    json!({
        "kind": kind,
        "connection_slug": string_field(&contract.source, &["connection_slug", "connectionSlug"]),
        "connector_slug": string_field(&contract.source, &["connector_slug", "connectorSlug"]),
        "summary": format!("{summary_label} from chat_id {chat_id}"),
        "delivery_target": {
            "type": "telegram_chat",
            "chat_id": chat_id,
            "chat_kind": chat_kind,
        },
        "sender": sender_from_source(&contract.source),
        "text": string_field(&contract.source, &["text", "message_text", "messageText"]),
        "message_id": contract
            .source
            .get("message_id")
            .or_else(|| contract.source.get("messageId"))
            .cloned(),
    })
}

fn gmail_source_context(contract: &MonitorContract) -> Value {
    json!({
        "kind": "gmail_message",
        "connection_slug": string_field(&contract.source, &["connection_slug", "connectionSlug"]),
        "connector_slug": string_field(&contract.source, &["connector_slug", "connectorSlug"]),
        "summary": string_field(&contract.source, &["subject"]).unwrap_or_else(|| {
            let thread_id = string_field(&contract.source, &["thread_id", "threadId"]).unwrap_or_default();
            format!("Gmail message in thread {thread_id}")
        }),
        "delivery_target": {
            "type": "gmail_thread",
            "account": string_field(&contract.source, &["account", "account_id", "accountId"]),
            "thread_id": string_field(&contract.source, &["thread_id", "threadId"]),
            "message_id": string_field(&contract.source, &["message_id", "messageId"]),
        },
        "sender": sender_from_source(&contract.source),
        "text": string_field(&contract.source, &["snippet", "text", "body"]),
    })
}

fn calendar_source_context(contract: &MonitorContract) -> Value {
    json!({
        "kind": "calendar_event",
        "connection_slug": string_field(&contract.source, &["connection_slug", "connectionSlug"]),
        "connector_slug": string_field(&contract.source, &["connector_slug", "connectorSlug"]),
        "summary": string_field(&contract.source, &["summary", "title"]).unwrap_or_else(|| "Calendar event".to_string()),
        "delivery_target": {
            "type": "calendar_event",
            "account": string_field(&contract.source, &["account", "account_id", "accountId"]),
            "calendar_id": string_field(&contract.source, &["calendar_id", "calendarId"]),
            "event_id": string_field(&contract.source, &["event_id", "eventId"]),
            "html_link": string_field(&contract.source, &["html_link", "htmlLink"]),
        },
        "sender": sender_from_source(&contract.source),
        "text": string_field(&contract.source, &["description", "text"]),
    })
}

fn generic_source_context(contract: &MonitorContract) -> Value {
    json!({
        "kind": "generic_review",
        "connection_slug": string_field(&contract.source, &["connection_slug", "connectionSlug"]),
        "connector_slug": string_field(&contract.source, &["connector_slug", "connectorSlug"]),
        "summary": string_field(&contract.source, &["summary", "subject", "title"]).unwrap_or_else(|| "Monitor item".to_string()),
        "sender": sender_from_source(&contract.source),
        "text": string_field(&contract.source, &["text", "body", "description"]),
    })
}

fn sender_from_source(source: &Map<String, Value>) -> Value {
    let mut sender = Map::new();
    if let Some(from) = source.get("from").and_then(Value::as_object) {
        for key in ["id", "name", "email", "username"] {
            if let Some(value) = non_empty_string(from.get(key)) {
                sender.insert(key.to_string(), Value::String(value));
            }
        }
    }
    for (field, keys) in [
        ("id", &["sender_id", "senderId", "from_id", "fromId"][..]),
        (
            "name",
            &["sender_name", "senderName", "from_name", "fromName"],
        ),
        (
            "email",
            &["from_email", "fromEmail", "sender_email", "senderEmail"],
        ),
        (
            "username",
            &[
                "sender_username",
                "senderUsername",
                "from_username",
                "fromUsername",
            ],
        ),
    ] {
        if !sender.contains_key(field) {
            if let Some(value) = string_field(source, keys) {
                sender.insert(field.to_string(), Value::String(value));
            }
        }
    }
    Value::Object(sender)
}

fn string_field(object: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| non_empty_string(object.get(*key)))
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn canonical_json(value: &Value) -> Result<String> {
    match value {
        Value::Null | Value::Bool(_) | Value::Number(_) | Value::String(_) => {
            serde_json::to_string(value).context("failed to encode canonical scalar")
        }
        Value::Array(values) => {
            let parts = values
                .iter()
                .map(canonical_json)
                .collect::<Result<Vec<_>>>()?;
            Ok(format!("[{}]", parts.join(",")))
        }
        Value::Object(object) => {
            let mut entries = object.iter().collect::<Vec<_>>();
            entries.sort_by(|(left, _), (right, _)| left.cmp(right));
            let mut parts = Vec::with_capacity(entries.len());
            for (key, value) in entries {
                let key = serde_json::to_string(key).context("failed to encode canonical key")?;
                parts.push(format!("{key}:{}", canonical_json(value)?));
            }
            Ok(format!("{{{}}}", parts.join(",")))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Map, Value};

    fn metadata(value: Value) -> Map<String, Value> {
        value.as_object().unwrap().clone()
    }

    #[test]
    fn parses_gmail_v2_contract_and_ignores_forged_display_context() {
        let metadata = metadata(json!({
            "source_context": {
                "kind": "telegram_direct_message",
                "delivery_target": {
                    "type": "telegram_chat",
                    "chat_id": "999"
                }
            },
            "monitor": {
                "schema_version": 2,
                "kind": "gmail.reply",
                "source": {
                    "connector_slug": "gmail-browser",
                    "connection_slug": "gmail-browser",
                    "account": "winterfell0614@gmail.com",
                    "thread_id": "thread-123",
                    "message_id": "message-123",
                    "from": {
                        "name": "Fu Xiangyu",
                        "email": "fuxiangyu@example.com"
                    }
                },
                "action": {
                    "type": "gmail_reply_draft",
                    "approval": "draft_then_create_gmail_draft"
                },
                "source_hash": "sha256:stale"
            }
        }));

        let contract = parse_monitor_contract(&metadata)
            .unwrap()
            .expect("contract");
        assert_eq!(contract.kind, MonitorTaskKind::GmailReply);

        let source_context = display_source_context(&contract);
        assert_eq!(source_context["kind"], "gmail_message");
        assert_eq!(source_context["delivery_target"]["type"], "gmail_thread");
        assert_eq!(source_context["delivery_target"]["thread_id"], "thread-123");
        assert_eq!(source_context["sender"]["email"], "fuxiangyu@example.com");
    }

    #[test]
    fn monitor_contract_hash_excludes_stored_hash_and_display_mirror() {
        let mut metadata = metadata(json!({
            "source_context": {
                "kind": "telegram_direct_message",
                "delivery_target": {
                    "type": "telegram_chat",
                    "chat_id": "999"
                }
            },
            "monitor": {
                "schema_version": 2,
                "kind": "gmail.reply",
                "source": {
                    "connector_slug": "gmail-browser",
                    "connection_slug": "gmail-browser",
                    "account": "winterfell0614@gmail.com",
                    "thread_id": "thread-123",
                    "message_id": "message-123",
                    "from": {
                        "name": "Fu Xiangyu",
                        "email": "fuxiangyu@example.com"
                    }
                },
                "action": {
                    "type": "gmail_reply_draft",
                    "approval": "draft_then_create_gmail_draft"
                },
                "source_hash": "sha256:stale"
            }
        }));

        let contract = parse_monitor_contract(&metadata)
            .unwrap()
            .expect("contract");
        assert_eq!(
            monitor_contract_hash(&contract).unwrap(),
            "sha256:b8e1bc99df97a47171b03fd10a708fb4c8220f8ae5cbe59e5c6ce4005cc847b2"
        );

        metadata.insert(
            "source_context".to_string(),
            json!({
                "kind": "telegram_direct_message",
                "delivery_target": {
                    "type": "telegram_chat",
                    "chat_id": "changed"
                }
            }),
        );
        metadata
            .get_mut("monitor")
            .and_then(Value::as_object_mut)
            .unwrap()
            .insert(
                "source_hash".to_string(),
                Value::String("sha256:changed".to_string()),
            );

        let contract = parse_monitor_contract(&metadata)
            .unwrap()
            .expect("contract");
        assert_eq!(
            monitor_contract_hash(&contract).unwrap(),
            "sha256:b8e1bc99df97a47171b03fd10a708fb4c8220f8ae5cbe59e5c6ce4005cc847b2"
        );
    }

    #[test]
    fn rejects_conflicting_top_level_typed_blocks() {
        let metadata = metadata(json!({
            "monitor": {
                "schema_version": 2,
                "kind": "gmail.reply",
                "source": {
                    "thread_id": "thread-123",
                    "message_id": "message-123"
                },
                "action": {
                    "type": "gmail_reply_draft"
                }
            },
            "telegram": {
                "chat_id": "42"
            }
        }));

        let error = parse_monitor_contract(&metadata).unwrap_err();
        assert!(error
            .to_string()
            .contains("conflicting typed monitor metadata"));
    }
}
