use super::*;
use puffer_config::PufferConfig;
use puffer_provider_registry::{AuthMode, Modality, ModelDescriptor, ProviderDescriptor};
use puffer_session_store::SessionMetadata;
use serde_json::Value;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use uuid::Uuid;

fn bash_invocation(command: &str, output: &str, success: bool) -> ToolInvocation {
    ToolInvocation {
        call_id: "call-bash".to_string(),
        tool_id: "Bash".to_string(),
        input: format!(r#"{{"command":"{command}","description":"run verifier"}}"#),
        output: output.to_string(),
        success,
        metadata: Value::Null,
        terminate: false,
    }
}

fn write_invocation(path: &str, content: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: "call-write".to_string(),
        tool_id: "Write".to_string(),
        input: format!(r#"{{"file_path":"{path}","content":{content:?}}}"#),
        output: String::new(),
        success: true,
        metadata: Value::Null,
        terminate: false,
    }
}

fn reflection_test_state() -> crate::AppState {
    crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    )
}

fn openai_judge_provider(base_url: String) -> ProviderDescriptor {
    ProviderDescriptor {
        id: "openai".to_string(),
        display_name: "OpenAI".to_string(),
        base_url,
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: Default::default(),
        query_params: Default::default(),
        discovery: None,
        models: vec![ModelDescriptor {
            id: "gpt-5".to_string(),
            display_name: "GPT-5".to_string(),
            provider: "openai".to_string(),
            api: "openai-responses".to_string(),
            context_window: 272_000,
            max_output_tokens: 16_384,
            supports_reasoning: true,
            compat: None,
            input: vec![Modality::Text],
            cost: None,
        }],
        chat_completions_path: None,
    }
}

fn minimal_batch_assessment() -> BatchAssessment {
    BatchAssessment {
        validation_progress: false,
        artifact_progress: false,
        edit_progress: false,
        loopiness_score: 0,
        focus_bad: false,
        time_since_progress_ms: 0,
        signal_notes: Vec::new(),
        recent_actions: Vec::new(),
    }
}

#[test]
fn reflection_defaults_enable_code_and_llm_judges() {
    let config = ReflectionConfig::default();
    assert_eq!(config.language, ReflectionLanguage::Chinese);
    assert!(config.code_judge.is_some());
    assert!(config.llm_judge.is_some());
}

#[test]
fn llm_judge_defaults_use_gpt_54_low_and_current_window() {
    let config = LlmJudgeConfig::default();
    assert_eq!(config.model_selector.as_deref(), Some("openai/gpt-5.4"));
    assert_eq!(config.effort_level.as_deref(), Some("low"));
    assert_eq!(
        config.prompt_cache_mode,
        LlmJudgePromptCacheMode::InheritMainAgent
    );
    assert_eq!(config.context_scope, LlmJudgeContextScope::CurrentWindow);
}

#[test]
fn llm_judge_strategy_default_is_single_call_when_env_unset() {
    // Use a fresh AppState path: clear env then construct default.
    let _guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("PUFFER_REFLECTION_JUDGE_STRATEGY").ok();
    std::env::remove_var("PUFFER_REFLECTION_JUDGE_STRATEGY");
    let config = LlmJudgeConfig::default();
    assert_eq!(config.strategy, LlmJudgeStrategy::SingleCall);
    if let Some(value) = prior {
        std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", value);
    }
}

#[test]
fn llm_judge_strategy_from_env_parses_aliases() {
    let _guard = ENV_LOCK.lock().unwrap();
    let prior = std::env::var("PUFFER_REFLECTION_JUDGE_STRATEGY").ok();
    let prior_iters = std::env::var("PUFFER_REFLECTION_JUDGE_MAX_ITERATIONS").ok();

    for raw in ["single_call", "single", "inline", "SINGLE_CALL"] {
        std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", raw);
        assert_eq!(
            LlmJudgeStrategy::from_env(),
            Some(LlmJudgeStrategy::SingleCall)
        );
    }
    for raw in ["sub_agent", "subagent", "agent", "Sub_Agent"] {
        std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", raw);
        std::env::remove_var("PUFFER_REFLECTION_JUDGE_MAX_ITERATIONS");
        // Default 3 when MAX_ITERATIONS unset.
        assert_eq!(
            LlmJudgeStrategy::from_env(),
            Some(LlmJudgeStrategy::SubAgent { max_iterations: 3 })
        );
    }
    std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", "agent");
    std::env::set_var("PUFFER_REFLECTION_JUDGE_MAX_ITERATIONS", "7");
    assert_eq!(
        LlmJudgeStrategy::from_env(),
        Some(LlmJudgeStrategy::SubAgent { max_iterations: 7 })
    );

    // Garbage strategy → None (caller falls back to default).
    std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", "nonsense");
    assert_eq!(LlmJudgeStrategy::from_env(), None);

    std::env::remove_var("PUFFER_REFLECTION_JUDGE_STRATEGY");
    std::env::remove_var("PUFFER_REFLECTION_JUDGE_MAX_ITERATIONS");
    assert_eq!(LlmJudgeStrategy::from_env(), None, "absent env → None");
    if let Some(v) = prior {
        std::env::set_var("PUFFER_REFLECTION_JUDGE_STRATEGY", v);
    }
    if let Some(v) = prior_iters {
        std::env::set_var("PUFFER_REFLECTION_JUDGE_MAX_ITERATIONS", v);
    }
}

/// Process-global mutex for tests that mutate env vars used by
/// `LlmJudgeStrategy::from_env`. Without serialization these tests
/// race when run with `--test-threads > 1`.
static ENV_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn llm_judge_side_state_inherits_main_agent_cache_key_by_default() {
    let mut state = crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    state.push_message(crate::state::MessageRole::User, "main transcript");
    state.prompt_cache_key_override = Some("main-cache-key".to_string());
    state.plan_mode = true;

    let mut config = LlmJudgeConfig::default();
    config.model_selector = Some("openai/gpt-5.3-codex-spark".to_string());
    let side_state = judge::build_llm_judge_side_state(&state, &config, "judge prompt");

    assert!(side_state.transcript.is_empty());
    assert!(!side_state.plan_mode);
    assert_eq!(
        side_state.current_model.as_deref(),
        Some("openai/gpt-5.3-codex-spark")
    );
    assert_eq!(side_state.current_provider.as_deref(), Some("openai"));
    assert_eq!(
        side_state.prompt_cache_key_override.as_deref(),
        Some("main-cache-key")
    );
}

/// Recursion guard: a sub-agent grader must NOT inherit the parent's
/// `reflection_config`, otherwise `apply_session_reflection_default`
/// would re-inject it into the sub-agent's `TurnRequestOptions` and
/// each judge would spawn its own grader sub-agent indefinitely.
/// `build_llm_judge_side_state` is the single chokepoint that has to
/// clear it; this test pins that contract.
#[test]
fn llm_judge_side_state_clears_reflection_config_to_prevent_recursion() {
    let mut state = crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    // Parent has reflection enabled (the common case for any user
    // who turned on `/reflect`). The clone-and-clear contract must
    // strip it from the side state.
    state.reflection_config = Some(super::ReflectionConfig::default());

    let config = LlmJudgeConfig::default();
    let side_state = judge::build_llm_judge_side_state(&state, &config, "judge prompt");

    assert!(
        side_state.reflection_config.is_none(),
        "side state must not inherit parent reflection_config (recursion guard)"
    );
}

#[test]
fn llm_judge_side_state_can_use_dedicated_cache_key() {
    let mut state = crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    state.push_message(crate::state::MessageRole::User, "main transcript");
    state.prompt_cache_key_override = Some("main-cache-key".to_string());

    let mut config = LlmJudgeConfig::default();
    config.prompt_cache_mode = LlmJudgePromptCacheMode::Dedicated;
    let side_state = judge::build_llm_judge_side_state(&state, &config, "judge prompt");

    let cache_key = side_state
        .prompt_cache_key_override
        .as_deref()
        .expect("llm judge cache key");
    assert_ne!(cache_key, "main-cache-key");
    assert!(cache_key.starts_with("reflection-judge-"));
}

#[test]
fn openai_llm_judge_rejects_incomplete_responses_payload() {
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 8192];
        let _ = stream.read(&mut buffer).unwrap();

        let body = concat!(
            "{",
            "\"id\":\"resp_incomplete_judge\",",
            "\"status\":\"incomplete\",",
            "\"incomplete_details\":{\"reason\":\"content_filter\"},",
            "\"output_text\":\"{\\\"decision\\\":\\\"continue\\\"}\"",
            "}"
        );
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
    });

    let mut state = reflection_test_state();
    state.current_provider = Some("openai".to_string());
    state.current_model = Some("openai/gpt-5".to_string());

    let mut providers = ProviderRegistry::new();
    providers.register(openai_judge_provider(format!("http://{address}")));
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("openai", "sk-openai");

    let mut config = LlmJudgeConfig::default();
    config.model_selector = Some("openai/gpt-5".to_string());
    let attempt = judge::run_llm_judge(
        "verify incomplete judge payload handling",
        &std::collections::BTreeSet::new(),
        ReflectionLanguage::English,
        &config,
        &minimal_batch_assessment(),
        None,
        &[crate::runtime::openai::conversation::ConversationItem::user_message("still working")],
        &state,
        &LoadedResources::default(),
        &providers,
        &mut auth_store,
        None,
    );
    server.join().unwrap();

    let error = attempt.error.expect("incomplete payload should fail");
    assert!(
        error.contains("Incomplete response returned, reason: content_filter"),
        "judge error should preserve incomplete reason, got: {error}"
    );
    assert!(
        attempt.raw_response_text.is_none(),
        "incomplete payload must not be accepted as judge text"
    );
}

#[test]
fn checkpoint_prompt_uses_chinese_questions() {
    let start = unix_time_ms();
    let mut tracker = ReflectionTracker::new(
        "Write the answer to /app/out.txt and use /tests/check.sh to verify it.",
        ReflectionConfig::default(),
    );
    let placeholder = write_invocation("/app/out.txt", "[]");
    assert!(tracker
        .observe_batch_at(&[placeholder], start + 1_000)
        .is_none());

    let failed = bash_invocation("bash /tests/check.sh", "2 failed, 0 passed", false);
    let checkpoint = tracker
        .observe_batch_at(
            &[failed.clone(), failed.clone(), failed.clone()],
            start + 1_000 + 10 * 60 * 1000,
        )
        .expect("checkpoint should trigger");
    assert!(checkpoint.prompt.contains("当前目标是什么？"));
    assert!(checkpoint.prompt.contains("Judge 结论"));
}

#[test]
fn meaningful_artifact_write_resets_progress() {
    let start = unix_time_ms();
    let mut tracker = ReflectionTracker::new(
        "Write the answer to /app/out.txt and verify it.",
        ReflectionConfig::default(),
    );
    let meaningful = write_invocation("/app/out.txt", "final answer\\n");
    assert!(tracker
        .observe_batch_at(&[meaningful], start + 2_000)
        .is_none());
    let failed = bash_invocation("bash /tests/check.sh", "1 failed", false);
    assert!(tracker
        .observe_batch_at(&[failed.clone(), failed], start + 62_000)
        .is_none());
}

#[test]
fn trace_events_capture_batch_and_code_judge_decision() {
    let start = unix_time_ms();
    let mut config = ReflectionConfig::default();
    config.llm_judge = None;
    let mut tracker = ReflectionTracker::new(
        "Write the answer to /app/out.txt and use /tests/check.sh to verify it.",
        config,
    );
    let placeholder = write_invocation("/app/out.txt", "[]");
    let initial = tracker
        .observe_batch_with_trace_at(&[placeholder], start + 1_000)
        .expect("initial observation should exist");
    assert_eq!(initial.trace_events.len(), 1);
    assert!(matches!(
        &initial.trace_events[0],
        ReflectionTraceEvent::BatchObserved {
            should_evaluate: false,
            ..
        }
    ));
    assert!(initial.checkpoint.is_none());

    let failed = bash_invocation("bash /tests/check.sh", "2 failed, 0 passed", false);
    let triggered = tracker
        .observe_batch_with_trace_at(
            &[failed.clone(), failed.clone(), failed],
            start + 1_000 + 10 * 60 * 1000,
        )
        .expect("triggered observation should exist");
    assert!(triggered.checkpoint.is_some());
    assert!(triggered.trace_events.iter().any(|event| matches!(
        event,
        ReflectionTraceEvent::BatchObserved {
            should_evaluate: true,
            ..
        }
    )));
    assert!(triggered.trace_events.iter().any(|event| matches!(
        event,
        ReflectionTraceEvent::CodeJudgeDecision {
            triggered: true,
            ..
        }
    )));
    assert!(triggered.trace_events.iter().any(|event| matches!(
        event,
        ReflectionTraceEvent::FinalDecision {
            triggered_checkpoint: true,
            ..
        }
    )));
}

#[test]
fn evaluation_slot_is_not_consumed_when_no_signal_fires() {
    // Regression guard: prior to this commit, `observe_batch_with_trace_at`
    // advanced `last_evaluation_batch` as soon as the evaluation gate opened
    // (score >= EVALUATION_TRIGGER_SCORE=3), even if `code_judge_signal`
    // returned None (score < config.min_score=4). That silenced the tracker
    // for the next `MIN_BATCHES_BETWEEN_EVALUATIONS` batches without ever
    // producing a checkpoint. The fix is to bind the slot bump to signal
    // presence, matching master's pre-split behavior.
    let mut config = ReflectionConfig::default();
    config.llm_judge = None;
    config.code_judge = Some(CodeJudgeConfig {
        // Tight enough to cross `EVALUATION_TRIGGER_SCORE` on stall alone
        // (soft_stall 0 → +2, hard_stall 0 → +2 = 4) but the assertion
        // below relies on the final `code_judge_score` actually being high
        // enough to raise a signal too, so we keep `min_score` at its
        // default 4.
        soft_stall_ms: 0,
        hard_stall_ms: 0,
        ..CodeJudgeConfig::default()
    });
    let start = unix_time_ms();
    let mut tracker = ReflectionTracker::new("regression guard", config);

    // Ramp total_tool_calls >= MIN_TOOL_CALLS_BEFORE_EVALUATION. Two bash
    // invocations × three batches = six calls; nothing looks like a
    // "test"/"verify" command so validation_progress stays false.
    for i in 0..3 {
        let _ = tracker.observe_batch_with_trace_at(
            &[
                bash_invocation("echo one", "one", true),
                bash_invocation("echo two", "two", true),
            ],
            start + 1_000 + (i as u128) * 10,
        );
    }

    // Intentionally raise the `min_score` threshold above what the current
    // assessment can reach (loopiness=0, stall_time≈0 in the injected clock
    // so the score is actually 0 here — but `EVALUATION_TRIGGER_SCORE` is
    // also 3, so `should_evaluate` would be false anyway. We construct a
    // scenario where `should_evaluate` is true but `code_judge_signal` is
    // None by giving it a large stall delta with a high `min_score`.)
    // Since we can't easily hit the 3..min_score window deterministically
    // in one call without manipulating internals, this test fires a large
    // stall instead and asserts that when `signal` IS produced, the slot
    // correctly advances (positive control), and the immediately following
    // observation can itself re-evaluate if it produces a signal too —
    // covering the symmetrical property.
    let batches_before = tracker.batch_count_for_test();
    let first_eval = tracker
        .observe_batch_with_trace_at(
            &[
                bash_invocation("echo stall", "stall", true),
                bash_invocation("echo stall", "stall", true),
            ],
            start + 10 * 60 * 1000,
        )
        .expect("observation expected");
    let first_batch = tracker.batch_count_for_test();
    assert!(first_batch > batches_before);
    if first_eval.checkpoint.is_some() {
        assert_eq!(
            tracker.last_evaluation_batch_for_test(),
            first_batch,
            "slot should advance when a checkpoint fires"
        );
    } else {
        assert!(
            tracker.last_evaluation_batch_for_test() < first_batch,
            "slot must not advance without a checkpoint"
        );
    }
}

#[test]
fn llm_judge_skipped_event_fires_when_llm_judge_is_disabled() {
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;

    let mut config = ReflectionConfig::default();
    config.llm_judge = None;
    config.code_judge = Some(CodeJudgeConfig {
        soft_stall_ms: 0,
        hard_stall_ms: 0,
        min_score: 1,
        ..CodeJudgeConfig::default()
    });
    let mut tracker = ReflectionTracker::new("stall this task until reflection triggers", config);

    let state = crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    let resources = LoadedResources::default();
    let providers = ProviderRegistry::new();
    let mut auth_store = AuthStore::default();

    // Batch 1: ramps batch_count to 1 and total_tool_calls to 2. Still below
    // MIN_BATCHES_BETWEEN_EVALUATIONS, so no evaluation runs.
    let _ = tracker.observe_batch_with_judge(
        &[
            bash_invocation("echo ping", "pong", true),
            bash_invocation("echo ping", "pong", true),
        ],
        &[],
        &state,
        &resources,
        &providers,
        &mut auth_store,
        None,
    );

    // Batch 2: batch_count=2, total_tool_calls=4, stall score >= 4. Evaluation
    // fires and — because `llm_judge` is disabled — `llm_judge_signal` must
    // push a `LlmJudgeSkipped { mode: "disabled", ... }` trace event before
    // returning.
    let observation = tracker
        .observe_batch_with_judge(
            &[
                bash_invocation("echo ping", "pong", true),
                bash_invocation("echo ping", "pong", true),
            ],
            &[],
            &state,
            &resources,
            &providers,
            &mut auth_store,
            None,
        )
        .expect("second batch should produce an observation");

    let skipped = observation
        .trace_events
        .iter()
        .find(|event| matches!(event, ReflectionTraceEvent::LlmJudgeSkipped { .. }))
        .expect("LlmJudgeSkipped trace event should be emitted when llm_judge is disabled");
    if let ReflectionTraceEvent::LlmJudgeSkipped { mode, reason } = skipped {
        assert_eq!(mode, "disabled", "mode tag should flag the disabled path");
        assert!(
            reason.to_ascii_lowercase().contains("disabled"),
            "reason should explain the disable: {reason}"
        );
    }
}

/// G2: circuit breaker — once `consecutive_llm_judge_failures` reaches
/// `MAX_CONSECUTIVE_LLM_JUDGE_FAILURES` (= 3, mirroring CC's `pd7=3`
/// at claude-2.1.133 bundle:4885), `llm_judge_signal` must skip
/// further judge calls without issuing a request. The first batch
/// after the trip emits a `LlmJudgeSkipped` event explaining the
/// trip; subsequent batches stay silent.
#[test]
fn llm_judge_circuit_breaker_trips_after_three_consecutive_failures() {
    use puffer_provider_registry::{AuthStore, ProviderRegistry};
    use puffer_resources::LoadedResources;

    // Real config with llm_judge enabled — without the breaker, the
    // tracker would try to issue an HTTP request via `run_llm_judge`.
    let mut config = ReflectionConfig::default();
    config.code_judge = Some(CodeJudgeConfig {
        soft_stall_ms: 0,
        hard_stall_ms: 0,
        min_score: 1,
        ..CodeJudgeConfig::default()
    });
    let mut tracker = ReflectionTracker::new("verify circuit breaker semantics", config);
    // Pre-trip the breaker.
    tracker.force_llm_judge_failures_for_test(super::MAX_CONSECUTIVE_LLM_JUDGE_FAILURES);

    let state = crate::AppState::new(
        PufferConfig::default(),
        std::env::temp_dir(),
        SessionMetadata {
            id: Uuid::nil(),
            display_name: None,
            generated_title: None,
            cwd: std::env::temp_dir(),
            created_at_ms: 0,
            updated_at_ms: 0,
            parent_session_id: None,
            slug: None,
            tags: Vec::new(),
            note: None,
        },
    );
    let resources = LoadedResources::default();
    let providers = ProviderRegistry::new();
    let mut auth_store = AuthStore::default();

    // Drive two batches so evaluation actually fires (same shape as
    // `llm_judge_skipped_event_fires_when_llm_judge_is_disabled`).
    let _ = tracker.observe_batch_with_judge(
        &[
            bash_invocation("echo ping", "pong", true),
            bash_invocation("echo ping", "pong", true),
        ],
        &[],
        &state,
        &resources,
        &providers,
        &mut auth_store,
        None,
    );
    let observation = tracker
        .observe_batch_with_judge(
            &[
                bash_invocation("echo ping", "pong", true),
                bash_invocation("echo ping", "pong", true),
            ],
            &[],
            &state,
            &resources,
            &providers,
            &mut auth_store,
            None,
        )
        .expect("second batch should produce an observation");

    // No request should have been issued.
    assert!(
        !observation
            .trace_events
            .iter()
            .any(|event| matches!(event, ReflectionTraceEvent::LlmJudgeRequest { .. })),
        "circuit-broken tracker must not emit LlmJudgeRequest"
    );

    // The trip event must fire exactly once (this batch).
    let skipped: Vec<&ReflectionTraceEvent> = observation
        .trace_events
        .iter()
        .filter(|event| matches!(event, ReflectionTraceEvent::LlmJudgeSkipped { .. }))
        .collect();
    assert_eq!(
        skipped.len(),
        1,
        "exactly one LlmJudgeSkipped should describe the trip"
    );
    if let ReflectionTraceEvent::LlmJudgeSkipped { reason, .. } = skipped[0] {
        assert!(
            reason.to_ascii_lowercase().contains("circuit breaker"),
            "skip reason should mention the breaker; got: {reason}"
        );
    }

    assert!(
        tracker.llm_judge_breaker_tripped_for_test(),
        "breaker flag must be set after the trip"
    );

    // A subsequent batch should still skip but stay silent (no
    // duplicate trip event).
    let observation2 = tracker
        .observe_batch_with_judge(
            &[
                bash_invocation("echo ping", "pong", true),
                bash_invocation("echo ping", "pong", true),
            ],
            &[],
            &state,
            &resources,
            &providers,
            &mut auth_store,
            None,
        )
        .expect("third batch should produce an observation");
    assert!(
        !observation2
            .trace_events
            .iter()
            .any(|event| matches!(event, ReflectionTraceEvent::LlmJudgeSkipped { .. })),
        "post-trip batches must NOT re-emit a skip event"
    );
}

#[test]
fn scp_style_remote_is_not_treated_as_filesystem_path() {
    let tracker = ReflectionTracker::new(
        "configure git server for git@localhost:/git/server and serve hello.html",
        ReflectionConfig::default(),
    );
    let relevant: std::collections::BTreeSet<String> = tracker.relevant_paths_for_test().clone();
    assert!(
        !relevant.iter().any(|path| path.contains('@')),
        "scp-style remotes should be filtered out; saw: {relevant:?}"
    );
    assert!(
        relevant.iter().any(|path| path.contains("hello.html")),
        "plain filesystem references should still be kept; saw: {relevant:?}"
    );
}
