use puffer_session_store::{GitDiffSnapshot, TranscriptEvent, TranscriptRewrite};

pub(crate) const ACTIVITY_IDLE: &str = "idle";
pub(crate) const ACTIVITY_RUNNING: &str = "running";
pub(crate) const ACTIVITY_AWAITING: &str = "awaiting";

/// Derives the coarse desktop activity state for a session list row.
pub(crate) fn session_activity_status(events: &[TranscriptEvent]) -> &'static str {
    let events = activity_events_after_rewrites(events);
    if latest_action_requires_permission(&events) {
        return ACTIVITY_AWAITING;
    }
    if latest_diff_has_changes(&events) {
        return ACTIVITY_RUNNING;
    }
    if latest_action_is_unanswered(&events) {
        return ACTIVITY_RUNNING;
    }
    ACTIVITY_IDLE
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

fn latest_action_requires_permission(events: &[TranscriptEvent]) -> bool {
    for event in events.iter().rev() {
        match event {
            TranscriptEvent::SystemMessage { text, .. } => {
                return text_requires_permission(text);
            }
            TranscriptEvent::ToolInvocation { output, .. } => {
                return output_requires_permission(output);
            }
            TranscriptEvent::UserMessage { .. }
            | TranscriptEvent::AssistantMessage { .. }
            | TranscriptEvent::CommandInvoked { .. }
            | TranscriptEvent::GitDiffSnapshot { .. } => return false,
            TranscriptEvent::SessionRenamed { .. }
            | TranscriptEvent::TranscriptRewritten { .. }
            | TranscriptEvent::StateSnapshot { .. } => {}
        }
    }
    false
}

fn latest_action_is_unanswered(events: &[TranscriptEvent]) -> bool {
    for event in events.iter().rev() {
        match event {
            TranscriptEvent::UserMessage { .. } | TranscriptEvent::CommandInvoked { .. } => {
                return true;
            }
            TranscriptEvent::AssistantMessage { .. }
            | TranscriptEvent::SystemMessage { .. }
            | TranscriptEvent::ToolInvocation { .. }
            | TranscriptEvent::GitDiffSnapshot { .. } => return false,
            TranscriptEvent::SessionRenamed { .. }
            | TranscriptEvent::TranscriptRewritten { .. }
            | TranscriptEvent::StateSnapshot { .. } => {}
        }
    }
    false
}

fn latest_diff_has_changes(events: &[TranscriptEvent]) -> bool {
    for event in events.iter().rev() {
        match event {
            TranscriptEvent::GitDiffSnapshot { snapshot } => {
                return diff_snapshot_has_changes(snapshot);
            }
            TranscriptEvent::TranscriptRewritten { .. }
            | TranscriptEvent::StateSnapshot { .. }
            | TranscriptEvent::SessionRenamed { .. } => {}
            _ => {}
        }
    }
    false
}

fn diff_snapshot_has_changes(snapshot: &GitDiffSnapshot) -> bool {
    [
        snapshot.status.as_str(),
        snapshot.unstaged_diffstat.as_str(),
        snapshot.staged_diffstat.as_str(),
        snapshot.patch.as_str(),
        snapshot.patch_excerpt.as_str(),
    ]
    .iter()
    .any(|value| !value.trim().is_empty())
}

fn text_requires_permission(text: &str) -> bool {
    parse_tool_output(text)
        .map(output_requires_permission)
        .unwrap_or_else(|| output_requires_permission(text))
}

fn parse_tool_output(text: &str) -> Option<&str> {
    let (_header, rest) = text.split_once('\n')?;
    let input = rest.strip_prefix("input: ")?;
    let (_input_text, output_text) = input.split_once('\n')?;
    Some(output_text)
}

fn output_requires_permission(output: &str) -> bool {
    output.trim().strip_prefix("Permission required:").is_some()
}

#[cfg(test)]
mod tests {
    use super::{session_activity_status, ACTIVITY_AWAITING, ACTIVITY_IDLE, ACTIVITY_RUNNING};
    use puffer_session_store::{GitDiffSnapshot, TranscriptEvent, TranscriptRewrite};

    #[test]
    fn detects_unanswered_user_turns_as_running() {
        let events = vec![TranscriptEvent::UserMessage {
            text: "continue".to_string(),
            actor: None,
        }];

        assert_eq!(session_activity_status(&events), ACTIVITY_RUNNING);
    }

    #[test]
    fn detects_permission_requests_as_awaiting() {
        let events = vec![TranscriptEvent::ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "Bash".to_string(),
            input: r#"{"command":"cargo test"}"#.to_string(),
            output: "Permission required: command needs approval".to_string(),
            success: false,
            actor: None,
            subject: None,
        }];

        assert_eq!(session_activity_status(&events), ACTIVITY_AWAITING);
    }

    #[test]
    fn detects_latest_dirty_diff_as_running() {
        let events = vec![
            TranscriptEvent::UserMessage {
                text: "edit".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "done".to_string(),
                actor: None,
            },
            TranscriptEvent::GitDiffSnapshot {
                snapshot: GitDiffSnapshot {
                    command: "git diff".to_string(),
                    status: " M src/main.rs".to_string(),
                    unstaged_diffstat: String::new(),
                    staged_diffstat: String::new(),
                    patch: String::new(),
                    patch_excerpt: String::new(),
                },
            },
        ];

        assert_eq!(session_activity_status(&events), ACTIVITY_RUNNING);
    }

    #[test]
    fn completed_turn_without_changes_is_idle() {
        let events = vec![
            TranscriptEvent::UserMessage {
                text: "hello".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "hi".to_string(),
                actor: None,
            },
        ];

        assert_eq!(session_activity_status(&events), ACTIVITY_IDLE);
    }

    #[test]
    fn transcript_clear_discards_unanswered_user_turn_status() {
        let events = vec![
            TranscriptEvent::UserMessage {
                text: "old prompt".to_string(),
                actor: None,
            },
            TranscriptEvent::TranscriptRewritten {
                rewrite: TranscriptRewrite::Clear,
            },
        ];

        assert_eq!(session_activity_status(&events), ACTIVITY_IDLE);
    }

    #[test]
    fn transcript_pop_discards_latest_unanswered_user_turn_status() {
        let events = vec![
            TranscriptEvent::UserMessage {
                text: "hello".to_string(),
                actor: None,
            },
            TranscriptEvent::AssistantMessage {
                text: "hi".to_string(),
                actor: None,
            },
            TranscriptEvent::UserMessage {
                text: "remove this".to_string(),
                actor: None,
            },
            TranscriptEvent::TranscriptRewritten {
                rewrite: TranscriptRewrite::PopLast { count: 1 },
            },
        ];

        assert_eq!(session_activity_status(&events), ACTIVITY_IDLE);
    }
}
