//! Spawn candidate-generation sub-agents.

use crate::{parse_skill_md, AgentRuntime, ExecutionTrace, SkillCandidate};
use anyhow::Result;

/// Builds the per-candidate generation prompt.
pub trait GeneratePromptBuilder: Send + Sync {
    /// Returns the prompt for one of N generation calls.
    ///
    /// `index` is the candidate position in `[0, n)` and may be used to inject
    /// diversity hints.
    fn build(&self, trace: &ExecutionTrace, index: usize) -> String;
}

/// Default inline-template generation prompt builder.
pub struct DefaultGeneratePrompt;

impl GeneratePromptBuilder for DefaultGeneratePrompt {
    fn build(&self, trace: &ExecutionTrace, index: usize) -> String {
        let trace_yaml = serde_yaml::to_string(trace).unwrap_or_default();
        let style = match index % 3 {
            0 => "Be concise and procedural.",
            1 => "Emphasize edge cases and pitfalls.",
            _ => "Emphasize when-to-use triggers and examples.",
        };
        format!(
            "You are generating a reusable SKILL.md from an execution trace.\n\
             Output ONLY a SKILL.md document with YAML frontmatter (name, description) followed\n\
             by sections: Overview, When to Use, Topic Sections, Common Pitfalls, Verification\n\
             Checklist. Stay under 15000 bytes. Style hint: {style}\n\n\
             Quality requirements:\n\
             - Make the skill conditional: include concrete task/domain triggers and when not to use it.\n\
             - Prefer a narrow domain skill over a catch-all benchmark skill. If the trace mixes\n\
               unrelated task families, choose the single highest-signal reusable workflow and\n\
               exclude the others in When to Use / non-goals.\n\
             - If the trace only teaches a lightweight artifact contract that future task prompts\n\
               already state exactly, generate a skill only when there is a non-obvious recovery\n\
               or verification method; otherwise this knowledge should remain memory-only.\n\
             - Make the skill verifier-first: preserve required output files, schemas, tests,\n\
               permissions, success signals, and minimal verification steps from the trace.\n\
             - Generalize temporary paths, run ids, model names, and incidental benchmark noise,\n\
               but keep durable commands or contracts needed to reproduce the outcome.\n\
             - Do not write broad productivity advice. A future agent must inspect the current\n\
               task prompt and verifier before applying this skill.\n\
             - If the skill could conflict with a future task's filenames, schema, or tests,\n\
               explicitly say the current task contract overrides the skill.\n\n\
             - Preserve \"looks done but verifier failed\" lessons when present: service tasks\n\
               may require a running service rather than a setup script; report tasks may\n\
               require exact JSON keys and CWE labels; denied shell commands may require\n\
               switching to Write/Edit plus smaller executable commands; and polyglot tasks\n\
               may require both language entrypoints to work.\n\n\
             EXECUTION TRACE (yaml):\n{trace_yaml}\n",
            style = style,
            trace_yaml = trace_yaml,
        )
    }
}

/// Spawns N generation calls and returns valid candidates.
///
/// Invalid frontmatter, oversize bodies, and runtime failures cause that
/// candidate to be discarded. Returns an error only if zero candidates remain.
pub async fn generate_candidates<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn GeneratePromptBuilder,
    trace: &ExecutionTrace,
    n: usize,
    max_size_bytes: usize,
) -> Result<Vec<SkillCandidate>> {
    let prompts: Vec<String> = (0..n).map(|index| builder.build(trace, index)).collect();
    let mut calls = Vec::new();
    for prompt in &prompts {
        calls.push(runtime.invoke_agent(prompt));
    }

    let mut candidates = Vec::new();
    for (index, call) in calls.into_iter().enumerate() {
        match call.await {
            Ok(raw) => match parse_skill_md(&raw) {
                Ok(candidate) if candidate.body.len() <= max_size_bytes => {
                    candidates.push(candidate);
                }
                Ok(_) => tracing::warn!(index, "candidate exceeded size budget"),
                Err(error) => {
                    tracing::warn!(index, error = %error, "invalid candidate frontmatter");
                }
            },
            Err(error) => tracing::warn!(index, error = %error, "generation runtime error"),
        }
    }
    if candidates.is_empty() {
        anyhow::bail!("all {} generation candidates failed", n);
    }
    Ok(candidates)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TraceEntry;
    use std::sync::Mutex;

    struct ScriptedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for ScriptedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn sample_trace() -> ExecutionTrace {
        ExecutionTrace {
            entries: vec![TraceEntry {
                summary: "did a thing".into(),
                tool_calls: vec!["bash".into()],
                succeeded: true,
            }],
            task_summary: "do the thing".into(),
        }
    }

    #[tokio::test]
    async fn generate_returns_valid_candidates() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let runtime = ScriptedRuntime(Mutex::new(vec![valid.into(), valid.into(), valid.into()]));
        let candidates =
            generate_candidates(&runtime, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000)
                .await
                .unwrap();
        assert_eq!(candidates.len(), 3);
    }

    #[tokio::test]
    async fn generate_discards_invalid_keeps_valid() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let runtime = ScriptedRuntime(Mutex::new(vec![
            "garbage".into(),
            valid.into(),
            "also garbage".into(),
        ]));
        let candidates =
            generate_candidates(&runtime, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000)
                .await
                .unwrap();
        assert_eq!(candidates.len(), 1);
    }

    #[tokio::test]
    async fn generate_errors_when_all_fail() {
        let runtime = ScriptedRuntime(Mutex::new(vec![
            "garbage".into(),
            "garbage".into(),
            "garbage".into(),
        ]));
        let result =
            generate_candidates(&runtime, &DefaultGeneratePrompt, &sample_trace(), 3, 15_000).await;
        assert!(result.is_err());
    }

    #[test]
    fn default_prompt_requires_conditional_verifier_first_skill() {
        let prompt = DefaultGeneratePrompt.build(&sample_trace(), 0);

        assert!(prompt.contains("Make the skill conditional"));
        assert!(prompt.contains("narrow domain skill"));
        assert!(prompt.contains("memory-only"));
        assert!(prompt.contains("Make the skill verifier-first"));
        assert!(prompt.contains("current task contract overrides the skill"));
        assert!(prompt.contains("looks done but verifier failed"));
    }
}
