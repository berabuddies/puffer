//! Telegram contact relationship analysis.
//!
//! Ranks the top-N 1-on-1 Telegram contacts by recent chat frequency (read from
//! the account's `message-diagnostics.ndjson`) and asks the local qwen35 model to
//! characterize each person's relationship with the user. Progress + the final
//! result are pushed to the frontend over the daemon event bus.
//!
//! Data source: batch read of the already-on-disk diagnostics file (cheap, no
//! live Telegram calls). Frequency: messages in a trailing window (default 90
//! days), counting only delivered 1-on-1 messages, ranked desc with ties broken
//! by most-recent activity.

use crate::daemon::ServerEnvelope;
use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::File;
use std::hash::{Hash, Hasher};
use std::io::{BufRead, BufReader};
use std::path::Path;
use tokio::sync::broadcast;

const WINDOW_DAYS: i64 = 90;
const TOP_N: usize = 5;
const EVENT_CHANNEL: &str = "telegram:relationships";
/// Cap transcript length fed to the model to keep the prompt small.
const MAX_TRANSCRIPT_MSGS: usize = 40;
/// Hard cap on transcript characters per contact (~a few hundred tokens). Guards
/// against flooders and keeps cloud cost trivial regardless of history size.
const MAX_TRANSCRIPT_CHARS: usize = 2400;

/// Which model backs the analysis. Cloud (default) is effectively free for this
/// workload (~3k tokens/contact); local is the privacy option.
pub(crate) struct ModelBackend {
    pub endpoint: String,
    pub model: String,
    /// `Some` for cloud (sent as a Bearer token); `None` for the local shim.
    pub api_key: Option<String>,
    /// Local qwen reads `enable_thinking`; cloud APIs reject unknown fields.
    pub local: bool,
}

impl ModelBackend {
    pub fn local() -> Self {
        Self {
            endpoint: "http://127.0.0.1:8088/v1/chat/completions".into(),
            model: "qwen3.5-0.8b".into(),
            api_key: None,
            local: true,
        }
    }

    pub fn cloud(api_key: String) -> Self {
        Self {
            endpoint: "https://api.openai.com/v1/chat/completions".into(),
            model: "gpt-5.4-mini".into(),
            api_key: Some(api_key),
            local: false,
        }
    }
}

/// One message extracted from the diagnostics log, minimal fields for ranking
/// and transcript rendering.
#[derive(Debug, Clone)]
struct DiagMessage {
    chat_id: i64,
    name: String,
    is_outgoing: bool,
    date_ms: i64,
    text: String,
}

/// A ranked contact plus the messages used to build its transcript.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ContactRank {
    pub chat_id: i64,
    pub name: String,
    pub message_count: usize,
    #[serde(skip)]
    messages: Vec<DiagMessage>,
}

/// The model's relationship verdict for one contact.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct RelationshipReport {
    pub chat_id: i64,
    pub name: String,
    pub message_count: usize,
    pub relationship: Option<String>,
    pub closeness: Option<i64>,
    pub tone: Option<String>,
    pub evidence: Option<String>,
}

/// Reads + ranks the top-N contacts from a diagnostics ndjson file. Pure (takes
/// `now_ms` explicitly) so it is unit-testable without a clock or network.
fn rank_contacts(path: &Path, now_ms: i64, window_days: i64, top_n: usize) -> Result<Vec<ContactRank>> {
    let file = File::open(path)
        .with_context(|| format!("open telegram diagnostics {}", path.display()))?;
    let cutoff = now_ms - window_days * 24 * 60 * 60 * 1000;

    let mut by_chat: HashMap<i64, ContactRank> = HashMap::new();
    for line in BufReader::new(file).lines() {
        let Ok(line) = line else { continue };
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let Ok(rec) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        // People only (1-on-1), delivered messages only (skip duplicate/suppressed),
        // within the trailing window.
        if rec.get("chat_kind").and_then(Value::as_str) != Some("user") {
            continue;
        }
        if rec.get("stage").and_then(Value::as_str) != Some("emitted") {
            continue;
        }
        let date_ms = rec.get("date_ms").and_then(Value::as_i64).unwrap_or(0);
        if date_ms < cutoff {
            continue;
        }
        let Some(chat_id) = rec.get("chat_id").and_then(Value::as_i64) else {
            continue;
        };
        let is_outgoing = rec.get("is_outgoing").and_then(Value::as_bool).unwrap_or(false);
        // For a 1-on-1 chat the contact's display name is the chat title; fall back
        // to the sender name for incoming messages.
        let name = rec
            .get("chat_title")
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
            .or_else(|| rec.get("sender_name").and_then(Value::as_str))
            .unwrap_or("")
            .to_string();
        let text = rec
            .get("text_prefix")
            .and_then(Value::as_str)
            .unwrap_or("")
            .trim()
            .to_string();
        // Default: ignore image/media-only messages. Telegram already maps
        // stickers/emoji to an emoji in the text, so those still count; a
        // caption-less photo has empty text and is skipped (no image analysis).
        if text.is_empty() {
            continue;
        }

        let entry = by_chat.entry(chat_id).or_insert_with(|| ContactRank {
            chat_id,
            name: name.clone(),
            message_count: 0,
            messages: Vec::new(),
        });
        entry.message_count += 1;
        if !name.is_empty() {
            entry.name = name; // prefer a non-empty, latest-seen name
        }
        entry.messages.push(DiagMessage {
            chat_id,
            name: entry.name.clone(),
            is_outgoing,
            date_ms,
            text,
        });
    }

    let mut ranked: Vec<ContactRank> = by_chat.into_values().collect();
    ranked.sort_by(|a, b| {
        b.message_count
            .cmp(&a.message_count)
            .then_with(|| latest(b).cmp(&latest(a)))
    });
    ranked.truncate(top_n);
    Ok(ranked)
}

fn latest(c: &ContactRank) -> i64 {
    c.messages.iter().map(|m| m.date_ms).max().unwrap_or(0)
}

/// Renders the contact's recent conversation as a "用户:/对方:" transcript.
/// Collapses flooder spam by dropping exact-duplicate message texts (hashed),
/// keeps the most recent messages, and hard-caps total characters so a single
/// contact can never blow up the prompt (or cloud cost).
fn transcript(contact: &ContactRank) -> String {
    let mut msgs = contact.messages.clone();
    msgs.sort_by_key(|m| m.date_ms);

    // De-dup identical texts (the "刷屏" case) by hash, keeping first occurrence.
    let mut seen: std::collections::HashSet<u64> = std::collections::HashSet::new();
    let mut deduped: Vec<&DiagMessage> = Vec::new();
    for m in &msgs {
        let mut h = std::collections::hash_map::DefaultHasher::new();
        m.text.hash(&mut h);
        if seen.insert(h.finish()) {
            deduped.push(m);
        }
    }

    // Most-recent MAX_TRANSCRIPT_MSGS, then trim oldest lines until under the
    // character cap.
    let start = deduped.len().saturating_sub(MAX_TRANSCRIPT_MSGS);
    let mut lines: Vec<String> = deduped[start..]
        .iter()
        .map(|m| {
            let who = if m.is_outgoing { "用户" } else { contact.name.as_str() };
            format!("{who}: {}", m.text)
        })
        .collect();
    while lines.len() > 1 && lines.iter().map(|l| l.chars().count() + 1).sum::<usize>() > MAX_TRANSCRIPT_CHARS {
        lines.remove(0);
    }
    lines.join("\n")
}

const SYSTEM_PROMPT: &str = "你是关系分析助手。根据用户与某联系人的聊天记录，判断两人关系。\
只输出一个 JSON 对象，字段：relationship(家人/恋人/朋友/同事或上司/商业服务/泛泛之交 其一)、\
closeness(1-5 整数)、tone(语气,简短)、evidence(一句依据)。只输出 JSON。";

/// Classifies one relationship via the chosen backend (local qwen no-think, or
/// a cloud OpenAI-compatible endpoint with a Bearer key).
fn analyze_contact(
    client: &reqwest::blocking::Client,
    backend: &ModelBackend,
    contact: &ContactRank,
) -> RelationshipReport {
    let user_prompt = format!(
        "联系人：{}\n消息数：{}\n\n聊天记录：\n{}",
        contact.name,
        contact.message_count,
        transcript(contact)
    );
    let mut body = json!({
        "model": backend.model,
        "max_tokens": 2048,
        "messages": [
            {"role": "system", "content": SYSTEM_PROMPT},
            {"role": "user", "content": user_prompt},
        ],
    });
    if backend.local {
        // Only the local qwen shim consumes this; cloud APIs reject unknown fields.
        body["enable_thinking"] = json!(false);
    }
    let mut req = client.post(&backend.endpoint).json(&body);
    if let Some(key) = &backend.api_key {
        req = req.bearer_auth(key);
    }
    let parsed = req
        .send()
        .and_then(|r| r.json::<Value>())
        .ok()
        .and_then(|resp| {
            resp.pointer("/choices/0/message/content")
                .and_then(Value::as_str)
                .map(|s| s.to_string())
        })
        .and_then(|content| extract_json_object(&content));

    RelationshipReport {
        chat_id: contact.chat_id,
        name: contact.name.clone(),
        message_count: contact.message_count,
        relationship: parsed
            .as_ref()
            .and_then(|p| p.get("relationship").and_then(Value::as_str).map(String::from)),
        closeness: parsed.as_ref().and_then(|p| p.get("closeness").and_then(Value::as_i64)),
        tone: parsed
            .as_ref()
            .and_then(|p| p.get("tone").and_then(Value::as_str).map(String::from)),
        evidence: parsed
            .as_ref()
            .and_then(|p| p.get("evidence").and_then(Value::as_str).map(String::from)),
    }
}

/// Pulls the last balanced-looking JSON object out of free model text
/// (strips any thinking prefix / code fences).
fn extract_json_object(text: &str) -> Option<Value> {
    let text = text.rsplit("</think>").next().unwrap_or(text);
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&text[start..=end]).ok()
}

/// Orchestrates ranking + per-contact analysis, emitting progress events and
/// returning the final result payload. `now_ms` is injected for testability.
pub(crate) fn run(
    diagnostics_path: &Path,
    events: &broadcast::Sender<ServerEnvelope>,
    connection_slug: &str,
    backend: &ModelBackend,
    now_ms: i64,
) -> Result<Value> {
    let emit = |phase: &str, payload: Value| {
        let _ = events.send(ServerEnvelope::Event {
            event: EVENT_CHANNEL.to_string(),
            payload: json!({
                "connectionSlug": connection_slug,
                "phase": phase,
                "data": payload,
            }),
        });
    };

    emit("ranking", json!({ "windowDays": WINDOW_DAYS, "model": backend.model }));
    let ranked = rank_contacts(diagnostics_path, now_ms, WINDOW_DAYS, TOP_N)?;
    emit(
        "ranked",
        json!({
            "contacts": ranked.iter().map(|c| json!({
                "chatId": c.chat_id, "name": c.name, "messageCount": c.message_count
            })).collect::<Vec<_>>()
        }),
    );

    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("build http client")?;

    let mut reports = Vec::with_capacity(ranked.len());
    for (idx, contact) in ranked.iter().enumerate() {
        emit(
            "analyzing",
            json!({ "index": idx + 1, "total": ranked.len(), "name": contact.name }),
        );
        let report = analyze_contact(&client, backend, contact);
        emit("analyzed", serde_json::to_value(&report).unwrap_or(Value::Null));
        reports.push(report);
    }

    let result = json!({
        "connectionSlug": connection_slug,
        "windowDays": WINDOW_DAYS,
        "reports": reports,
    });
    emit("done", result.clone());
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn write_diag(lines: &[Value]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for l in lines {
            writeln!(f, "{l}").unwrap();
        }
        f
    }

    fn msg(chat_id: i64, kind: &str, stage: &str, title: &str, outgoing: bool, date_ms: i64) -> Value {
        json!({
            "stage": stage, "chat_kind": kind, "chat_id": chat_id, "chat_title": title,
            "is_outgoing": outgoing, "date_ms": date_ms, "text_prefix": "hi"
        })
    }

    fn msg_text(chat_id: i64, text: &str, date_ms: i64) -> Value {
        json!({
            "stage": "emitted", "chat_kind": "user", "chat_id": chat_id, "chat_title": "Z",
            "is_outgoing": false, "date_ms": date_ms, "text_prefix": text
        })
    }

    const DAY: i64 = 24 * 60 * 60 * 1000;

    #[test]
    fn skips_media_only_messages() {
        let now = 1_000 * DAY;
        let lines = vec![
            msg_text(1, "hi", now - 1 * DAY),
            msg_text(1, "", now - 1 * DAY),     // photo, no caption
            msg_text(1, "   ", now - 1 * DAY),  // whitespace only
        ];
        let f = write_diag(&lines);
        let ranked = rank_contacts(f.path(), now, 90, 5).unwrap();
        assert_eq!(ranked[0].message_count, 1, "media/empty-text messages are ignored");
    }

    #[test]
    fn transcript_dedups_flooder_spam() {
        let now = 1_000 * DAY;
        let mut lines: Vec<Value> = (0..30).map(|i| msg_text(1, "在吗", now - 30 * DAY + i)).collect();
        lines.push(msg_text(1, "周末一起吃饭吧", now));
        let f = write_diag(&lines);
        let ranked = rank_contacts(f.path(), now, 90, 5).unwrap();
        let t = transcript(&ranked[0]);
        assert_eq!(t.matches("在吗").count(), 1, "repeated spam collapsed to one line");
        assert!(t.contains("周末一起吃饭吧"), "unique message kept");
    }

    #[test]
    fn ranks_one_on_one_by_recent_frequency() {
        let now = 1_000 * DAY;
        let lines = vec![
            // Alice: 3 recent
            msg(1, "user", "emitted", "Alice", false, now - 1 * DAY),
            msg(1, "user", "emitted", "Alice", true, now - 2 * DAY),
            msg(1, "user", "emitted", "Alice", false, now - 3 * DAY),
            // Bob: 2 recent
            msg(2, "user", "emitted", "Bob", false, now - 1 * DAY),
            msg(2, "user", "emitted", "Bob", false, now - 2 * DAY),
            // a group chat with many msgs -> excluded
            msg(9, "group", "emitted", "Team", false, now - 1 * DAY),
            msg(9, "group", "emitted", "Team", false, now - 1 * DAY),
            msg(9, "group", "emitted", "Team", false, now - 1 * DAY),
        ];
        let f = write_diag(&lines);
        let ranked = rank_contacts(f.path(), now, 90, 5).unwrap();
        assert_eq!(ranked.len(), 2, "groups excluded");
        assert_eq!(ranked[0].name, "Alice");
        assert_eq!(ranked[0].message_count, 3);
        assert_eq!(ranked[1].name, "Bob");
    }

    #[test]
    fn excludes_out_of_window_and_non_emitted() {
        let now = 1_000 * DAY;
        let lines = vec![
            msg(1, "user", "emitted", "Alice", false, now - 1 * DAY),
            msg(1, "user", "emitted", "Alice", false, now - 200 * DAY), // too old
            msg(1, "user", "duplicate", "Alice", false, now - 1 * DAY), // not delivered
            msg(1, "user", "suppressed", "Alice", false, now - 1 * DAY), // not delivered
        ];
        let f = write_diag(&lines);
        let ranked = rank_contacts(f.path(), now, 90, 5).unwrap();
        assert_eq!(ranked.len(), 1);
        assert_eq!(ranked[0].message_count, 1, "only the in-window emitted message counts");
    }

    #[test]
    fn truncates_to_top_n() {
        let now = 1_000 * DAY;
        let mut lines = Vec::new();
        for chat in 1..=8 {
            for _ in 0..chat {
                lines.push(msg(chat, "user", "emitted", &format!("c{chat}"), false, now - 1 * DAY));
            }
        }
        let f = write_diag(&lines);
        let ranked = rank_contacts(f.path(), now, 90, 5).unwrap();
        assert_eq!(ranked.len(), 5);
        assert_eq!(ranked[0].message_count, 8, "highest-frequency first");
    }

    #[test]
    fn extract_json_object_strips_thinking_and_fences() {
        let v = extract_json_object("blah</think>\n```json\n{\"relationship\":\"朋友\"}\n```").unwrap();
        assert_eq!(v.get("relationship").unwrap(), "朋友");
    }
}
