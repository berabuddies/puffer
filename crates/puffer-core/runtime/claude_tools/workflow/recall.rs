//! `Recall` workflow tool — auxiliary context with replay.
//!
//! Long agent runs compact/summarize their context, losing exact detail. Recall
//! is a retrieval layer over the session's own transcript: given a query it
//! lexically ranks past messages and REPLAYS the most relevant ones (verbatim)
//! back into the turn — so the model can recover an earlier diff, tool output,
//! or decision that fell out of the live window. (cf. MemoRAG / Memex(RL)
//! indexed-experience replay; the daemon already has a *live* replay ring — this
//! is the durable, query-driven counterpart.)

use crate::state::RenderedMessage;
use crate::AppState;
use anyhow::Result;
use serde_json::{json, Value};
use std::path::Path;

fn tokenize(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .filter(|t| t.len() >= 2)
        .map(|t| t.to_string())
        .collect()
}

/// Searchable text for a message: its text + tool name/input (so a query can
/// hit a past tool call, not just prose).
fn message_text(m: &RenderedMessage) -> String {
    let mut s = m.text.clone();
    if let Some(t) = &m.tool_id {
        s.push(' ');
        s.push_str(t);
    }
    if let Some(i) = &m.tool_input {
        s.push(' ');
        s.push_str(i);
    }
    s
}

/// BM25-lite over the transcript. df-weighted term overlap with mild length
/// normalization. Returns (index, score) sorted desc, score>0 only.
fn rank(transcript: &[RenderedMessage], query: &str) -> Vec<(usize, f64)> {
    let q: Vec<String> = tokenize(query);
    if q.is_empty() || transcript.is_empty() {
        return Vec::new();
    }
    let docs: Vec<Vec<String>> = transcript
        .iter()
        .map(|m| tokenize(&message_text(m)))
        .collect();
    let n = docs.len() as f64;
    let avg_len = docs.iter().map(|d| d.len()).sum::<usize>() as f64 / n.max(1.0);
    // document frequency per query term
    let mut scored: Vec<(usize, f64)> = Vec::new();
    let (k1, b) = (1.2_f64, 0.6_f64);
    for (i, d) in docs.iter().enumerate() {
        if d.is_empty() {
            continue;
        }
        let dl = d.len() as f64;
        let mut score = 0.0;
        for term in &q {
            let tf = d.iter().filter(|w| *w == term).count() as f64;
            if tf == 0.0 {
                continue;
            }
            let df = docs.iter().filter(|dd| dd.contains(term)).count() as f64;
            let idf = ((n - df + 0.5) / (df + 0.5) + 1.0).ln();
            score += idf * (tf * (k1 + 1.0)) / (tf + k1 * (1.0 - b + b * dl / avg_len.max(1.0)));
        }
        if score > 0.0 {
            scored.push((i, score));
        }
    }
    scored.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    scored
}

fn snippet(m: &RenderedMessage, max: usize) -> String {
    let role = format!("{:?}", m.role).to_lowercase();
    let mut body = m.text.trim().to_string();
    if body.is_empty() {
        if let Some(t) = &m.tool_id {
            body = format!(
                "{}({})",
                t,
                m.tool_input
                    .as_deref()
                    .unwrap_or("")
                    .chars()
                    .take(80)
                    .collect::<String>()
            );
        }
    }
    if body.chars().count() > max {
        body = body.chars().take(max).collect::<String>() + " …";
    }
    format!("[{role}] {body}")
}

/// Executes the `Recall` tool. Input: { query: string, k?: number }.
pub fn execute_recall(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let _ = cwd;
    let query = input
        .get("query")
        .and_then(Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if query.is_empty() {
        anyhow::bail!("Recall requires a non-empty 'query'");
    }
    let k = input
        .get("k")
        .and_then(Value::as_u64)
        .unwrap_or(5)
        .clamp(1, 20) as usize;

    // Don't rank the last message (the Recall call's own context) into results.
    let n = state.transcript.len();
    let cutoff = n.saturating_sub(1);
    let ranked = rank(&state.transcript[..cutoff], &query);

    if ranked.is_empty() {
        return Ok(serde_json::to_string_pretty(&json!({
            "query": query, "matches": 0,
            "note": "No relevant earlier context found in this session's transcript."
        }))?);
    }
    let hits: Vec<Value> = ranked
        .iter()
        .take(k)
        .map(|(i, score)| {
            json!({
                "turn": i,
                "score": (score * 100.0).round() / 100.0,
                "content": snippet(&state.transcript[*i], 600),
            })
        })
        .collect();
    Ok(serde_json::to_string_pretty(&json!({
        "query": query,
        "matches": ranked.len(),
        "replayed": hits.len(),
        "note": "Earlier context replayed from this session's transcript (most relevant first).",
        "results": hits,
    }))?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::state::{MessageRole, RenderedMessage};
    use puffer_config::PufferConfig;
    use puffer_session_store::SessionMetadata;
    use uuid::Uuid;

    fn msg(role: MessageRole, text: &str) -> RenderedMessage {
        RenderedMessage {
            role,
            text: text.to_string(),
            thinking: None,
            call_id: None,
            tool_id: None,
            tool_input: None,
            success: None,
        }
    }

    fn state_with(msgs: Vec<RenderedMessage>) -> AppState {
        let cwd = std::env::temp_dir();
        let session = SessionMetadata {
            id: Uuid::new_v4(),
            display_name: None,
            generated_title: None,
            cwd: cwd.clone(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        };
        let mut s = AppState::new(PufferConfig::default(), cwd, session);
        s.transcript = msgs;
        s
    }

    #[test]
    fn recall_finds_relevant_earlier_message() {
        let mut s = state_with(vec![
            msg(
                MessageRole::User,
                "let's set up the postgres connection pool with sqlx",
            ),
            msg(
                MessageRole::Assistant,
                "I configured the rate limiter middleware",
            ),
            msg(MessageRole::User, "now add the redis cache layer"),
            msg(MessageRole::Assistant, "done"), // last msg, excluded
        ]);
        let cwd = s.cwd.clone();
        let out = execute_recall(
            &mut s,
            &cwd,
            json!({ "query": "postgres database pool", "k": 2 }),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(v["matches"].as_u64().unwrap() >= 1, "found a match");
        let top = v["results"][0]["content"].as_str().unwrap();
        assert!(
            top.contains("postgres"),
            "most relevant is the postgres message, got: {top}"
        );
    }

    #[test]
    fn recall_empty_query_errors() {
        let mut s = state_with(vec![msg(MessageRole::User, "hi")]);
        let cwd = s.cwd.clone();
        assert!(execute_recall(&mut s, &cwd, json!({ "query": "  " })).is_err());
    }
}
