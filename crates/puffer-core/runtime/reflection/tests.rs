use super::*;

fn bash_invocation(command: &str, output: &str, success: bool) -> ToolInvocation {
    ToolInvocation {
        call_id: "call-bash".to_string(),
        tool_id: "Bash".to_string(),
        input: format!(r#"{{"command":"{command}","description":"run verifier"}}"#),
        output: output.to_string(),
        success,
    }
}

fn write_invocation(path: &str, content: &str) -> ToolInvocation {
    ToolInvocation {
        call_id: "call-write".to_string(),
        tool_id: "Write".to_string(),
        input: format!(r#"{{"file_path":"{path}","content":{content:?}}}"#),
        output: String::new(),
        success: true,
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
    assert_eq!(config.context_scope, LlmJudgeContextScope::CurrentWindow);
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
