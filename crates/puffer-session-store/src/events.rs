use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Describes one append-only transcript rewrite operation.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "operation", rename_all = "snake_case")]
pub enum TranscriptRewrite {
    Clear,
    PopLast {
        #[serde(default = "default_pop_count")]
        count: usize,
    },
}

fn default_pop_count() -> usize {
    1
}

/// Stores one append-only git diff snapshot for a command checkpoint.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct GitDiffSnapshot {
    pub command: String,
    pub status: String,
    pub unstaged_diffstat: String,
    pub staged_diffstat: String,
    #[serde(default)]
    pub patch: String,
    pub patch_excerpt: String,
}

/// Stores Claude-style read metadata that must survive session restore.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct ClaudeReadSnapshotEvent {
    pub path: String,
    pub timestamp_ms: u128,
    pub is_partial_view: bool,
}

/// Identifies the actor that produced or owns a transcript message.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct MessageActor {
    pub kind: MessageActorKind,
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub team_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<Uuid>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_session_id: Option<Uuid>,
}

/// Coarse category for a transcript actor.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MessageActorKind {
    User,
    Assistant,
    Agent,
    Subagent,
    TeamLead,
    System,
    Runtime,
    #[serde(other)]
    Unknown,
}

/// Stores a transcript event in append-only session history.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TranscriptEvent {
    UserMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    AssistantMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    SystemMessage {
        text: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    /// Structured tool invocation — preserves call_id, tool name, input/output
    /// so the Responses API can reconstruct FunctionCall/FunctionCallOutput
    /// items instead of degrading to system messages.
    ToolInvocation {
        call_id: String,
        tool_id: String,
        input: String,
        output: String,
        success: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        metadata: Option<Value>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        subject: Option<MessageActor>,
    },
    CommandInvoked {
        name: String,
        args: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        actor: Option<MessageActor>,
    },
    SessionRenamed {
        name: String,
    },
    GitDiffSnapshot {
        snapshot: GitDiffSnapshot,
    },
    TranscriptRewritten {
        #[serde(flatten)]
        rewrite: TranscriptRewrite,
    },
    /// Stores a persisted UI/runtime snapshot for session resume.
    ///
    /// This is not a full `AppState` dump. Session permissions are serialized
    /// here only through a legacy-compatible projection so resume can restore
    /// the subset of grants representable as session-wide allow-all plus
    /// whole-tool approvals.
    StateSnapshot {
        current_model: Option<String>,
        current_provider: Option<String>,
        theme: String,
        prompt_color: String,
        effort_level: String,
        fast_mode: bool,
        #[serde(default)]
        plan_mode: bool,
        #[serde(default)]
        plan_mode_attachment_turns: usize,
        #[serde(default)]
        plan_mode_attachment_count: usize,
        #[serde(default)]
        plan_mode_has_exited: bool,
        #[serde(default)]
        plan_mode_needs_reentry_attachment: bool,
        #[serde(default)]
        plan_mode_needs_exit_attachment: bool,
        sandbox_mode: String,
        remote_name: Option<String>,
        remote_environment: Option<String>,
        #[serde(default)]
        remote_session_id: Option<String>,
        #[serde(default)]
        remote_session_url: Option<String>,
        #[serde(default)]
        remote_session_status: Option<String>,
        #[serde(default)]
        active_team_name: Option<String>,
        statusline_enabled: bool,
        working_dirs: Vec<String>,
        #[serde(default)]
        claude_read_state: Vec<ClaudeReadSnapshotEvent>,
        #[serde(default)]
        session_allow_all: bool,
        #[serde(default)]
        session_tool_permissions: std::collections::HashMap<String, String>,
    },
}

#[cfg(test)]
mod tests {
    use super::{MessageActor, MessageActorKind, TranscriptEvent};

    #[test]
    fn user_message_without_actor_deserializes_for_old_sessions() {
        let event: TranscriptEvent =
            serde_json::from_str(r#"{"type":"user_message","text":"hello"}"#).unwrap();
        assert_eq!(
            event,
            TranscriptEvent::UserMessage {
                text: "hello".to_string(),
                actor: None,
            }
        );
    }

    #[test]
    fn changed_variants_without_actor_deserialize_for_old_sessions() {
        let assistant: TranscriptEvent =
            serde_json::from_str(r#"{"type":"assistant_message","text":"hello"}"#).unwrap();
        assert_eq!(
            assistant,
            TranscriptEvent::AssistantMessage {
                text: "hello".to_string(),
                actor: None,
            }
        );

        let system: TranscriptEvent =
            serde_json::from_str(r#"{"type":"system_message","text":"note"}"#).unwrap();
        assert_eq!(
            system,
            TranscriptEvent::SystemMessage {
                text: "note".to_string(),
                actor: None,
            }
        );

        let tool: TranscriptEvent = serde_json::from_str(
            r#"{"type":"tool_invocation","call_id":"call-1","tool_id":"Read","input":"{}","output":"ok","success":true}"#,
        )
        .unwrap();
        assert_eq!(
            tool,
            TranscriptEvent::ToolInvocation {
                call_id: "call-1".to_string(),
                tool_id: "Read".to_string(),
                input: "{}".to_string(),
                output: "ok".to_string(),
                success: true,
                metadata: None,
                actor: None,
                subject: None,
            }
        );

        let command: TranscriptEvent =
            serde_json::from_str(r#"{"type":"command_invoked","name":"help","args":""}"#).unwrap();
        assert_eq!(
            command,
            TranscriptEvent::CommandInvoked {
                name: "help".to_string(),
                args: String::new(),
                actor: None,
            }
        );
    }

    #[test]
    fn unknown_actor_kind_deserializes_for_future_compatibility() {
        let event: TranscriptEvent = serde_json::from_str(
            r#"{"type":"assistant_message","text":"hello","actor":{"kind":"external_agent","id":"agent-x"}}"#,
        )
        .unwrap();
        let TranscriptEvent::AssistantMessage {
            actor: Some(actor), ..
        } = event
        else {
            panic!("expected assistant message with actor");
        };
        assert_eq!(actor.kind, MessageActorKind::Unknown);
        assert_eq!(actor.id, "agent-x");
    }

    #[test]
    fn message_actor_round_trips_on_tool_invocation() {
        let actor = MessageActor {
            kind: MessageActorKind::Subagent,
            id: "agent-1".to_string(),
            agent_id: Some("agent-1".to_string()),
            agent_type: Some("reviewer".to_string()),
            name: Some("Reviewer".to_string()),
            team_name: Some("team".to_string()),
            session_id: None,
            parent_session_id: None,
        };
        let event = TranscriptEvent::ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "Read".to_string(),
            input: "{}".to_string(),
            output: "ok".to_string(),
            success: true,
            metadata: None,
            actor: Some(actor),
            subject: None,
        };

        let encoded = serde_json::to_string(&event).unwrap();
        assert!(encoded.contains(r#""agentId":"agent-1""#));
        let decoded: TranscriptEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, event);
    }

    #[test]
    fn tool_invocation_metadata_round_trips() {
        let event = TranscriptEvent::ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "ToolSearch".to_string(),
            input: r#"{"query":"skills"}"#.to_string(),
            output: "ok".to_string(),
            success: true,
            metadata: Some(serde_json::json!({
                "lambda_skill": {
                    "event": "host_call_committed",
                    "host_tool": "formal_search"
                }
            })),
            actor: None,
            subject: None,
        };

        let encoded = serde_json::to_string(&event).unwrap();
        assert!(encoded.contains(r#""metadata""#));
        let decoded: TranscriptEvent = serde_json::from_str(&encoded).unwrap();
        assert_eq!(decoded, event);
    }
}
