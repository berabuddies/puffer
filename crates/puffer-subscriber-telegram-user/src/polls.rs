//! Telegram poll rendering and vote selector helpers.

use std::collections::BTreeMap;

use anyhow::{anyhow, Result};
use grammers_client::types::media::Poll;
use serde_json::{json, Value};

#[derive(Debug, Clone)]
struct VoteSummary {
    chosen: bool,
    correct: bool,
    voters: i32,
}

/// Builds compact poll text suitable for succinct message search output.
pub(crate) fn poll_text(poll: &Poll) -> String {
    let question = text_with_entities_text(poll.question());
    let summaries = vote_summaries(poll);
    let answers = poll
        .iter_answers()
        .enumerate()
        .map(|(index, answer)| {
            let text = text_with_entities_text(&answer.text);
            let suffix = summaries
                .get(&hex_encode(&answer.option))
                .filter(|summary| summary.voters > 0)
                .map(|summary| format!(" ({} votes)", summary.voters))
                .unwrap_or_default();
            format!("{index}: {text}{suffix}")
        })
        .filter(|value| !value.is_empty())
        .collect::<Vec<_>>();
    let state = if poll.closed() { "closed" } else { "open" };
    let header = match poll.total_voters() {
        Some(total) => format!("poll: {question} [{state}, {total} voters]"),
        None => format!("poll: {question} [{state}]"),
    };
    if answers.is_empty() {
        header
    } else {
        format!("{header} | {}", answers.join(" / "))
    }
}

/// Builds full structured poll metadata for message search output.
pub(crate) fn poll_payload(poll: &Poll) -> Value {
    let summaries = vote_summaries(poll);
    let answers = poll
        .iter_answers()
        .enumerate()
        .map(|(index, answer)| {
            let option_hex = hex_encode(&answer.option);
            let summary = summaries.get(&option_hex);
            json!({
                "index": index,
                "text": text_with_entities_text(&answer.text),
                "option": option_hex,
                "option_hex": option_hex,
                "chosen": summary.map(|value| value.chosen).unwrap_or(false),
                "correct": summary.map(|value| value.correct).unwrap_or(false),
                "voters": summary.map(|value| value.voters),
            })
        })
        .collect::<Vec<_>>();
    json!({
        "question": text_with_entities_text(poll.question()),
        "is_quiz": poll.is_quiz(),
        "closed": poll.closed(),
        "total_voters": poll.total_voters(),
        "answers": answers,
    })
}

/// Resolves user-facing answer selectors into Telegram poll option bytes.
pub(crate) fn resolve_poll_options(poll: &Poll, selectors: &[String]) -> Result<Vec<Vec<u8>>> {
    if selectors.is_empty() {
        anyhow::bail!("poll vote requires at least one answer selector");
    }
    let answers = poll.iter_answers().collect::<Vec<_>>();
    let mut resolved = Vec::with_capacity(selectors.len());
    for selector in selectors {
        let trimmed = selector.trim();
        if trimmed.is_empty() {
            anyhow::bail!("poll vote selector must not be empty");
        }
        if let Some(option) = resolve_by_index(&answers, trimmed)? {
            resolved.push(option);
            continue;
        }
        if let Some(option) = resolve_by_text(&answers, trimmed)? {
            resolved.push(option);
            continue;
        }
        if let Some(bytes) = decode_option_token(trimmed)? {
            if answers.iter().any(|answer| answer.option == bytes) {
                resolved.push(bytes);
                continue;
            }
            anyhow::bail!("poll option token `{trimmed}` is not present on this poll");
        }
        anyhow::bail!("poll answer `{trimmed}` did not match any option");
    }
    Ok(resolved)
}

/// Encodes Telegram poll option bytes as lowercase hexadecimal text.
pub(crate) fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

/// Returns the visible string from Telegram's text-with-entities wrapper.
pub(crate) fn text_with_entities_text(value: &grammers_tl_types::enums::TextWithEntities) -> &str {
    match value {
        grammers_tl_types::enums::TextWithEntities::Entities(value) => value.text.as_str(),
    }
}

fn vote_summaries(poll: &Poll) -> BTreeMap<String, VoteSummary> {
    let mut summaries = BTreeMap::new();
    if let Some(results) = poll.iter_voters_summary() {
        for result in results {
            summaries.insert(
                hex_encode(&result.option),
                VoteSummary {
                    chosen: result.chosen,
                    correct: result.correct,
                    voters: result.voters,
                },
            );
        }
    }
    summaries
}

fn resolve_by_index(
    answers: &[&grammers_tl_types::types::PollAnswer],
    selector: &str,
) -> Result<Option<Vec<u8>>> {
    let Ok(index) = selector.parse::<usize>() else {
        return Ok(None);
    };
    let Some(answer) = answers.get(index) else {
        anyhow::bail!("poll answer index {index} is out of range");
    };
    Ok(Some(answer.option.clone()))
}

fn resolve_by_text(
    answers: &[&grammers_tl_types::types::PollAnswer],
    selector: &str,
) -> Result<Option<Vec<u8>>> {
    let normalized = selector.to_lowercase();
    let mut matches = answers
        .iter()
        .filter(|answer| text_with_entities_text(&answer.text).to_lowercase() == normalized)
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(None),
        1 => Ok(Some(matches.remove(0).option.clone())),
        _ => Err(anyhow!("poll answer text `{selector}` is ambiguous")),
    }
}

fn decode_option_token(selector: &str) -> Result<Option<Vec<u8>>> {
    let token = selector
        .strip_prefix("hex:")
        .or_else(|| selector.strip_prefix("option:"))
        .or_else(|| selector.strip_prefix("option_hex:"))
        .unwrap_or(selector);
    if token.len() % 2 != 0 || !token.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Ok(None);
    }
    let mut output = Vec::with_capacity(token.len() / 2);
    for chunk in token.as_bytes().chunks(2) {
        let hi = hex_nibble(chunk[0])?;
        let lo = hex_nibble(chunk[1])?;
        output.push((hi << 4) | lo);
    }
    Ok(Some(output))
}

fn hex_nibble(byte: u8) -> Result<u8> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err(anyhow!("invalid hex digit")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_and_decode_round_trip() {
        let encoded = hex_encode(&[0, 1, 10, 255]);
        assert_eq!(encoded, "00010aff");
        assert_eq!(
            decode_option_token("hex:00010aff").unwrap().unwrap(),
            vec![0, 1, 10, 255]
        );
        assert!(decode_option_token("not-hex").unwrap().is_none());
    }
}
