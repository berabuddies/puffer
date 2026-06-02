//! Stale desktop turn recovery helpers.

use puffer_session_store::{
    MessageActor, MessageActorKind, SessionRecord, TranscriptEvent, TranscriptRewrite,
};
use sha2::{Digest, Sha256};

pub(crate) const DEFAULT_STALE_TURN_RETRY_AFTER_MS: u64 = 120_000;

const RETRY_MARKER_PREFIX: &str = "Puffer recovery: retrying interrupted prompt.";
const EXHAUSTED_MARKER_PREFIX: &str =
    "Puffer recovery: interrupted prompt was already retried automatically.";
const PROMPT_HASH_LABEL: &str = "prompt_sha256:";

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum StaleTurnRecoveryDecision {
    Retry { message: String, marker: String },
    AlreadyRetried { marker: String },
    NotRecoverable,
}

/// Builds the system actor used for persisted recovery marker messages.
pub(crate) fn recovery_actor() -> MessageActor {
    MessageActor {
        kind: MessageActorKind::System,
        id: "puffer-recovery".to_string(),
        agent_id: None,
        agent_type: None,
        name: Some("Puffer recovery".to_string()),
        team_name: None,
        session_id: None,
        parent_session_id: None,
    }
}

/// Decides whether a loaded session should get one automatic stale-turn retry.
pub(crate) fn stale_turn_recovery_decision(
    record: &SessionRecord,
    now_ms: u64,
    retry_after_ms: u64,
) -> StaleTurnRecoveryDecision {
    if now_ms.saturating_sub(record.metadata.updated_at_ms) < retry_after_ms {
        return StaleTurnRecoveryDecision::NotRecoverable;
    }

    let events = activity_events_after_rewrites(&record.events);
    let Some(message) = latest_unanswered_user_message(&events) else {
        return StaleTurnRecoveryDecision::NotRecoverable;
    };
    let message = message.trim();
    if message.is_empty() {
        return StaleTurnRecoveryDecision::NotRecoverable;
    }

    let hash = prompt_hash(message);
    if has_retry_marker(&events, &hash) {
        return StaleTurnRecoveryDecision::AlreadyRetried {
            marker: exhausted_marker(&hash),
        };
    }

    StaleTurnRecoveryDecision::Retry {
        message: message.to_string(),
        marker: retry_marker(&hash),
    }
}

fn activity_events_after_rewrites(events: &[TranscriptEvent]) -> Vec<TranscriptEvent> {
    let mut projected = Vec::new();
    for event in events {
        match event {
            TranscriptEvent::TranscriptRewritten { rewrite } => {
                apply_activity_rewrite(&mut projected, rewrite);
            }
            TranscriptEvent::StateSnapshot { .. } => {}
            _ => projected.push(event.clone()),
        }
    }
    projected
}

fn apply_activity_rewrite(events: &mut Vec<TranscriptEvent>, rewrite: &TranscriptRewrite) {
    match rewrite {
        TranscriptRewrite::Clear => events.clear(),
        TranscriptRewrite::PopLast { count } => {
            for _ in 0..*count {
                if events.pop().is_none() {
                    break;
                }
            }
        }
    }
}

fn latest_unanswered_user_message(events: &[TranscriptEvent]) -> Option<&str> {
    for event in events.iter().rev() {
        match event {
            TranscriptEvent::UserMessage { text, .. } => return Some(text),
            TranscriptEvent::SessionRenamed { .. }
            | TranscriptEvent::TranscriptRewritten { .. }
            | TranscriptEvent::StateSnapshot { .. } => {}
            _ => return None,
        }
    }
    None
}

fn has_retry_marker(events: &[TranscriptEvent], hash: &str) -> bool {
    events.iter().any(|event| match event {
        TranscriptEvent::SystemMessage { text, .. } => {
            text.starts_with(RETRY_MARKER_PREFIX)
                && text
                    .lines()
                    .any(|line| line.trim() == format!("{PROMPT_HASH_LABEL} {hash}"))
        }
        _ => false,
    })
}

fn retry_marker(hash: &str) -> String {
    format!("{RETRY_MARKER_PREFIX}\n{PROMPT_HASH_LABEL} {hash}\nretry: automatic-once")
}

fn exhausted_marker(hash: &str) -> String {
    format!("{EXHAUSTED_MARKER_PREFIX}\n{PROMPT_HASH_LABEL} {hash}\nretry: manual-required")
}

fn prompt_hash(message: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(message.as_bytes());
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::{
        stale_turn_recovery_decision, StaleTurnRecoveryDecision, DEFAULT_STALE_TURN_RETRY_AFTER_MS,
    };
    use puffer_session_store::{SessionMetadata, SessionRecord, TranscriptEvent};
    use std::path::PathBuf;
    use uuid::Uuid;

    fn record(updated_at_ms: u64, events: Vec<TranscriptEvent>) -> SessionRecord {
        SessionRecord {
            metadata: SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: PathBuf::from("/tmp/project"),
                created_at_ms: updated_at_ms,
                updated_at_ms,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
            events,
        }
    }

    #[test]
    fn retries_old_unanswered_user_message_once() {
        let record = record(
            1_000,
            vec![TranscriptEvent::UserMessage {
                text: "pull".to_string(),
                actor: None,
            }],
        );

        let decision =
            stale_turn_recovery_decision(&record, 1_000 + DEFAULT_STALE_TURN_RETRY_AFTER_MS, 1);

        let StaleTurnRecoveryDecision::Retry { message, marker } = decision else {
            panic!("expected retry decision");
        };
        assert_eq!(message, "pull");
        assert!(marker.contains("prompt_sha256:"));
    }

    #[test]
    fn skips_recent_unanswered_user_message() {
        let record = record(
            1_000,
            vec![TranscriptEvent::UserMessage {
                text: "pull".to_string(),
                actor: None,
            }],
        );

        let decision = stale_turn_recovery_decision(
            &record,
            1_000 + DEFAULT_STALE_TURN_RETRY_AFTER_MS - 1,
            DEFAULT_STALE_TURN_RETRY_AFTER_MS,
        );

        assert_eq!(decision, StaleTurnRecoveryDecision::NotRecoverable);
    }

    #[test]
    fn marks_already_retried_prompt_for_manual_follow_up() {
        let first = record(
            1_000,
            vec![TranscriptEvent::UserMessage {
                text: "pull".to_string(),
                actor: None,
            }],
        );
        let StaleTurnRecoveryDecision::Retry { marker, .. } =
            stale_turn_recovery_decision(&first, 2_000, 1)
        else {
            panic!("expected first retry");
        };
        let second = record(
            2_000,
            vec![
                TranscriptEvent::UserMessage {
                    text: "pull".to_string(),
                    actor: None,
                },
                TranscriptEvent::SystemMessage {
                    text: marker,
                    actor: None,
                },
                TranscriptEvent::UserMessage {
                    text: "pull".to_string(),
                    actor: None,
                },
            ],
        );

        let decision = stale_turn_recovery_decision(&second, 3_000, 1);

        let StaleTurnRecoveryDecision::AlreadyRetried { marker } = decision else {
            panic!("expected already retried decision");
        };
        assert!(marker.contains("manual-required"));
    }

    #[test]
    fn ignores_answered_turns() {
        let record = record(
            1_000,
            vec![
                TranscriptEvent::UserMessage {
                    text: "hello".to_string(),
                    actor: None,
                },
                TranscriptEvent::AssistantMessage {
                    text: "hi".to_string(),
                    actor: None,
                },
            ],
        );

        let decision = stale_turn_recovery_decision(&record, 10_000, 1);

        assert_eq!(decision, StaleTurnRecoveryDecision::NotRecoverable);
    }
}
