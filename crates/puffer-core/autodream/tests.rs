use super::*;
use puffer_config::{set_puffer_home_override, ConfigPaths, MemoryConfig, PufferConfig};
use puffer_session_store::SessionMetadata;
use std::path::PathBuf;
use uuid::Uuid;

fn state_with_autodream(interval: usize) -> AppState {
    let mut state = AppState::new(
        PufferConfig {
            memory: MemoryConfig {
                autodream_enabled: true,
                autodream_interval: interval,
                ..MemoryConfig::default()
            },
            ..PufferConfig::default()
        },
        PathBuf::from("/workspace/demo"),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: PathBuf::from("/workspace/demo"),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    state.project_memory = Some(crate::memory::ProjectMemoryContext {
        project_name: "demo".to_string(),
        project_root: PathBuf::from("/workspace/demo"),
        memory_file: PathBuf::from("/tmp/MEMORY.md"),
        char_limit: 6_000,
    });
    state
}

fn temp_store() -> (tempfile::TempDir, SessionStore) {
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths {
        workspace_root: temp.path().join("workspace"),
        workspace_config_dir: temp.path().join("workspace/.puffer"),
        user_config_dir: temp.path().join("home/.puffer"),
        builtin_resources_dir: temp.path().join("resources"),
    };
    let store = SessionStore::from_paths(&paths).unwrap();
    (temp, store)
}

#[test]
fn genskill_marker_accepts_yes_only() {
    assert!(parse_genskill_marker("done\nAUTODREAM_GENSKILL: yes"));
    assert!(!parse_genskill_marker("AUTODREAM_GENSKILL: no"));
}

#[test]
fn autodream_counter_triggers_at_interval() {
    let mut state = state_with_autodream(2);
    assert!(!autodream_turn_completed(&mut state));
    assert!(autodream_turn_completed(&mut state));
    assert_eq!(state.autodream_review_turns, 0);
}

#[test]
fn autodream_counter_does_not_trigger_when_disabled() {
    let mut state = state_with_autodream(1);
    state.config.memory.autodream_enabled = false;

    assert!(!autodream_turn_completed(&mut state));
    assert_eq!(state.autodream_review_turns, 0);
}

#[test]
fn autodream_counter_does_not_trigger_without_project_memory() {
    let mut state = state_with_autodream(1);
    state.project_memory = None;

    assert!(!autodream_turn_completed(&mut state));
    assert_eq!(state.autodream_review_turns, 0);
}

#[test]
fn manual_autodream_bootstrap_registers_project_memory() {
    let temp = tempfile::tempdir().unwrap();
    let home = temp.path().join("home");
    let cwd = temp.path().join("workspace/demo");
    std::fs::create_dir_all(&cwd).unwrap();
    let _home = set_puffer_home_override(&home);
    let mut state = AppState::new(
        PufferConfig {
            memory: MemoryConfig {
                enabled: true,
                autodream_enabled: true,
                ..MemoryConfig::default()
            },
            ..PufferConfig::default()
        },
        cwd.clone(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd,
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );

    assert!(state.project_memory.is_none());
    let bootstrap = ensure_manual_autodream_project_memory(&mut state).unwrap();

    assert!(bootstrap.initialized_project_memory);
    assert!(bootstrap.message.contains("Initialized project memory at"));
    let project_memory = state.project_memory.expect("project memory");
    assert_eq!(project_memory.project_name, "demo");
    assert!(project_memory.memory_file.exists());
}

#[test]
fn autodream_gated_counter_skips_until_enough_sessions() {
    let (_temp, store) = temp_store();
    let mut state = state_with_autodream(1);
    state.config.memory.autodream_min_hours = 0;
    state.config.memory.autodream_min_sessions = 2;
    assert!(!autodream_turn_completed_with_store(&mut state, &store));
    assert_eq!(
        state.autodream_last_skip_reason.as_deref(),
        Some("session_gate: 0/2")
    );
}

#[test]
fn autodream_gated_counter_throttles_repeated_session_scans() {
    let (_temp, store) = temp_store();
    let mut state = state_with_autodream(1);
    state.config.memory.autodream_min_hours = 0;
    state.config.memory.autodream_min_sessions = 2;

    assert!(!autodream_turn_completed_with_store(&mut state, &store));
    assert_eq!(
        state.autodream_last_skip_reason.as_deref(),
        Some("session_gate: 0/2")
    );

    assert!(!autodream_turn_completed_with_store(&mut state, &store));
    assert_eq!(
        state.autodream_last_skip_reason.as_deref(),
        Some("scan_throttle")
    );
}

#[test]
fn autodream_gated_counter_runs_after_session_gate() {
    let (_temp, store) = temp_store();
    let mut state = state_with_autodream(1);
    state.config.memory.autodream_min_hours = 0;
    state.config.memory.autodream_min_sessions = 1;
    let other = store
        .create_session(PathBuf::from("/workspace/demo"))
        .unwrap();
    store
        .append_event(
            other.id,
            puffer_session_store::TranscriptEvent::UserMessage {
                text: "hello".to_string(),
                actor: None,
            },
        )
        .unwrap();
    assert!(autodream_turn_completed_with_store(&mut state, &store));
    assert_eq!(state.autodream_last_skip_reason, None);
}

#[test]
fn autodream_lock_allows_only_one_holder() {
    let (_temp, store) = temp_store();
    let first = acquire_autodream_lock(store.root()).unwrap();
    assert!(first.is_some());
    let second = acquire_autodream_lock(store.root()).unwrap();
    assert!(second.is_none());
}

#[test]
fn autodream_status_includes_original_mechanism_gates() {
    let state = state_with_autodream(2);
    let status = autodream_status(&state);
    assert!(status.contains("min_hours=24"));
    assert!(status.contains("min_sessions=5"));
    assert!(status.contains("last_skip_reason=none"));
}

#[test]
fn recent_session_selector_excludes_current_and_other_cwd() {
    let (_temp, store) = temp_store();
    let mut state = state_with_autodream(1);
    state.session = store
        .create_session(PathBuf::from("/workspace/demo"))
        .unwrap();
    let same = store
        .create_session(PathBuf::from("/workspace/demo"))
        .unwrap();
    let other = store
        .create_session(PathBuf::from("/workspace/other"))
        .unwrap();
    store
        .append_event(
            same.id,
            puffer_session_store::TranscriptEvent::UserMessage {
                text: "same cwd".to_string(),
                actor: None,
            },
        )
        .unwrap();
    store
        .append_event(
            other.id,
            puffer_session_store::TranscriptEvent::UserMessage {
                text: "other cwd".to_string(),
                actor: None,
            },
        )
        .unwrap();
    let selected = select_recent_autodream_sessions(&state, &store, 0).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].id, same.id);
}

#[test]
fn session_context_redacts_and_truncates_snippets() {
    let (_temp, store) = temp_store();
    let mut state = state_with_autodream(1);
    state.session = store
        .create_session(PathBuf::from("/workspace/demo"))
        .unwrap();
    let other = store
        .create_session(PathBuf::from("/workspace/demo"))
        .unwrap();
    store
        .append_event(
            other.id,
            puffer_session_store::TranscriptEvent::ToolInvocation {
                call_id: "call-1".to_string(),
                tool_id: "Bash".to_string(),
                input: "echo password=hunter2 token=abc sk-test".to_string(),
                output: "x".repeat(400),
                success: false,
                metadata: None,
                actor: None,
                subject: None,
            },
        )
        .unwrap();
    let context = build_recent_session_context_pack(&state, &store)
        .unwrap()
        .text;
    assert!(context.contains("<redacted>"));
    assert!(!context.contains("hunter2"));
    assert!(!context.contains("sk-test"));
    assert!(context.contains("..."));
}

#[test]
fn prompt_includes_recent_session_context_when_available() {
    let prompt = autodream_prompt_with_session_context(Some("- session demo\n  - user: hi"));
    assert!(prompt.contains("Recent session context since the last successful AutoDream"));
    assert!(prompt.contains("user: hi"));
}

#[test]
fn autodream_run_status_round_trips() {
    let (_temp, store) = temp_store();
    let status = AutoDreamRunStatusFile {
        status: "completed".to_string(),
        started_at_ms: 10,
        updated_at_ms: 20,
        sessions_reviewed: 3,
        tool_calls: 2,
        genskill_suggested: true,
        summary: "memory updated".to_string(),
        error: None,
        memory_changes: vec![AutoDreamMemoryChange {
            action: "add".to_string(),
            content: Some("Durable workflow memory".to_string()),
            old_text: None,
            success: true,
            message: "Added memory entry".to_string(),
        }],
        genskill_suggestion: Some(AutoDreamGenskillSuggestion {
            id: "autodream-20".to_string(),
            created_at_ms: 20,
            rationale: "Durable workflow captured".to_string(),
            memory_changes: 1,
            status: "pending_review".to_string(),
        }),
    };
    write_autodream_run_status(store.root(), &status).unwrap();
    assert_eq!(
        read_autodream_run_status(store.root()).unwrap(),
        Some(status)
    );
}

#[test]
fn autodream_status_with_store_reports_background_status() {
    let (_temp, store) = temp_store();
    let state = state_with_autodream(2);
    write_autodream_run_status(
        store.root(),
        &AutoDreamRunStatusFile {
            status: "failed".to_string(),
            started_at_ms: 10,
            updated_at_ms: 20,
            sessions_reviewed: 1,
            tool_calls: 0,
            genskill_suggested: false,
            summary: "AutoDream background consolidation failed".to_string(),
            error: Some("provider unavailable".to_string()),
            memory_changes: Vec::new(),
            genskill_suggestion: None,
        },
    )
    .unwrap();
    let status = autodream_status_with_store(&state, &store);
    assert!(status.contains("background_status=failed"));
    assert!(status.contains("background_sessions_reviewed=1"));
    assert!(status.contains("background_summary=AutoDream background consolidation failed"));
    assert!(status.contains("background_memory_changes=0"));
}

#[test]
fn autodream_status_with_store_reports_memory_changes_and_suggestion() {
    let (_temp, store) = temp_store();
    let state = state_with_autodream(2);
    write_autodream_run_status(
        store.root(),
        &AutoDreamRunStatusFile {
            status: "completed".to_string(),
            started_at_ms: 10,
            updated_at_ms: 20,
            sessions_reviewed: 2,
            tool_calls: 3,
            genskill_suggested: true,
            summary: "memory updated".to_string(),
            error: None,
            memory_changes: vec![AutoDreamMemoryChange {
                action: "add".to_string(),
                content: Some("Puffer workflow: verify then update memory".to_string()),
                old_text: None,
                success: true,
                message: "Added memory entry".to_string(),
            }],
            genskill_suggestion: Some(AutoDreamGenskillSuggestion {
                id: "autodream-20".to_string(),
                created_at_ms: 20,
                rationale: "workflow captured".to_string(),
                memory_changes: 1,
                status: "pending_review".to_string(),
            }),
        },
    )
    .unwrap();

    let status = autodream_status_with_store(&state, &store);

    assert!(status.contains("background_status=completed"));
    assert!(status.contains("background_memory_changes=1"));
    assert!(status.contains("background_memory_change=add success=true"));
    assert!(status.contains("background_genskill_suggestion_id=autodream-20"));
    assert!(status.contains("background_genskill_suggestion_status=pending_review"));
}

#[test]
fn memory_change_summary_extracts_memory_tool_inputs() {
    let changes = extract_memory_changes(&[ToolInvocation {
        call_id: "call-1".to_string(),
        tool_id: "Memory".to_string(),
        input: r#"{"action":"replace","old_text":"old secret=abc","content":"new durable"}"#
            .to_string(),
        output: r#"{"success":true,"message":"Replaced memory entry"}"#.to_string(),
        success: true,
        metadata: serde_json::Value::Null,
        terminate: false,
    }]);
    assert_eq!(changes.len(), 1);
    assert_eq!(changes[0].action, "replace");
    assert_eq!(changes[0].content.as_deref(), Some("new durable"));
    assert_eq!(changes[0].old_text.as_deref(), Some("old <redacted>"));
    assert_eq!(changes[0].message, "Replaced memory entry");
}

#[test]
fn memory_change_summary_ignores_non_memory_and_malformed_inputs() {
    let changes = extract_memory_changes(&[
        ToolInvocation {
            call_id: "call-1".to_string(),
            tool_id: "Read".to_string(),
            input: r#"{"file_path":"/tmp/MEMORY.md"}"#.to_string(),
            output: "contents".to_string(),
            success: true,
            metadata: serde_json::Value::Null,
            terminate: false,
        },
        ToolInvocation {
            call_id: "call-2".to_string(),
            tool_id: "Memory".to_string(),
            input: "not-json".to_string(),
            output: r#"{"success":false,"error":"invalid"}"#.to_string(),
            success: false,
            metadata: serde_json::Value::Null,
            terminate: false,
        },
        ToolInvocation {
            call_id: "call-3".to_string(),
            tool_id: "Memory".to_string(),
            input: r#"{"content":"missing action"}"#.to_string(),
            output: r#"{"success":false,"error":"missing action"}"#.to_string(),
            success: false,
            metadata: serde_json::Value::Null,
            terminate: false,
        },
    ]);

    assert!(changes.is_empty());
}

#[test]
fn genskill_suggestion_is_created_only_for_positive_marker() {
    let positive = AutoDreamOutcome {
        assistant_text: "Captured reusable workflow.\nAUTODREAM_GENSKILL: yes".to_string(),
        tool_invocations: Vec::new(),
        genskill_suggested: true,
    };
    let negative = AutoDreamOutcome {
        assistant_text: "Only ordinary facts.\nAUTODREAM_GENSKILL: no".to_string(),
        tool_invocations: Vec::new(),
        genskill_suggested: false,
    };

    let suggestion = build_genskill_suggestion(42, &positive, 1).unwrap();

    assert_eq!(suggestion.id, "autodream-42");
    assert_eq!(suggestion.status, "pending_review");
    assert_eq!(suggestion.memory_changes, 1);
    assert!(build_genskill_suggestion(43, &negative, 0).is_none());
}

#[test]
fn autodream_suggestions_reports_pending_genskill_review() {
    let (_temp, store) = temp_store();
    write_autodream_run_status(
        store.root(),
        &AutoDreamRunStatusFile {
            status: "completed".to_string(),
            started_at_ms: 10,
            updated_at_ms: 20,
            sessions_reviewed: 1,
            tool_calls: 1,
            genskill_suggested: true,
            summary: "workflow captured".to_string(),
            error: None,
            memory_changes: Vec::new(),
            genskill_suggestion: Some(AutoDreamGenskillSuggestion {
                id: "autodream-20".to_string(),
                created_at_ms: 20,
                rationale: "workflow captured".to_string(),
                memory_changes: 0,
                status: "pending_review".to_string(),
            }),
        },
    )
    .unwrap();
    let suggestions = autodream_suggestions_with_store(&store);
    assert!(suggestions.contains("id=autodream-20"));
    assert!(suggestions.contains("status=pending_review"));
}

#[test]
fn autodream_suggestions_reports_none_without_pending_review() {
    let (_temp, store) = temp_store();

    let suggestions = autodream_suggestions_with_store(&store);

    assert_eq!(suggestions, "AutoDream GenSkill suggestions: none");
}
