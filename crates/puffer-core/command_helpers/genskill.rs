//! Handler for the `/genskill` slash command.
//!
//! Builds an `ExecutionTrace` from the current `AppState::transcript`, invokes
//! `puffer-skill-evolution::run_gepa`, and writes the resulting SKILL.md to
//! `resources/skills/<name>/SKILL.md`.

use crate::{AppState, MessageRole};
use anyhow::{anyhow, Context, Result};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use puffer_skill_evolution::{
    run_gepa, AgentRuntime, ExecutionTrace, GepaOptions, SkillCandidate, TranscriptStep,
};
use std::fs;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Mutex;

struct PufferAgentRuntime {
    state: Mutex<AppState>,
    resources: LoadedResources,
    providers: ProviderRegistry,
    auth_store: Mutex<AuthStore>,
}

#[async_trait::async_trait]
impl AgentRuntime for PufferAgentRuntime {
    async fn invoke_agent(&self, prompt: &str) -> Result<String> {
        run_blocking_outside_tokio(|| {
            let mut state = self.state.lock().expect("genskill state lock poisoned");
            let mut auth_store = self
                .auth_store
                .lock()
                .expect("genskill auth store lock poisoned");
            let turn = crate::runtime::execute_user_prompt(
                &mut state,
                &self.resources,
                &self.providers,
                &mut auth_store,
                prompt,
            )?;
            Ok(turn.assistant_text)
        })
    }
}

/// Implementation of the `/genskill` local command.
///
/// Reads optional `--candidates N` and `--rounds K` flags from `args`. On
/// success, returns a user-facing message containing the path to the newly
/// written skill file.
pub(crate) fn handle_genskill_command(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<String> {
    let opts = parse_args(args)?;
    let trace = build_trace_from_state(state)?;
    let tool_call_count = state
        .transcript
        .iter()
        .filter(|message| matches!(message.role, MessageRole::ToolCall))
        .count();
    let has_reference_solution_context = state.transcript.iter().any(|message| {
        matches!(message.role, MessageRole::User)
            && message.text.contains("Reference solution context")
            && message.text.contains("Reference fix patch:")
    });
    if tool_call_count < 5 && !has_reference_solution_context {
        return Ok(
            "/genskill needs a substantive transcript (at least 5 tool calls). Use it after a non-trivial task."
                .to_string(),
        );
    }

    let runtime = PufferAgentRuntime {
        state: Mutex::new(state.clone()),
        resources: resources.clone(),
        providers: providers.clone(),
        auth_store: Mutex::new(auth_store.clone()),
    };
    let candidate = block_on_genskill_future(run_gepa(
        &runtime,
        &trace,
        &opts,
        &puffer_skill_evolution::DefaultGeneratePrompt,
        &puffer_skill_evolution::DefaultJudgePrompt,
        &puffer_skill_evolution::DefaultMutatePrompt,
    ))?;

    let path = write_skill_to_disk(&candidate)?;
    Ok(format!("Skill written to {}", path.display()))
}

fn block_on_genskill_future<F, T>(future: F) -> Result<T>
where
    F: Future<Output = Result<T>>,
{
    if let Ok(handle) = tokio::runtime::Handle::try_current() {
        return tokio::task::block_in_place(|| handle.block_on(future));
    }

    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("creating genskill runtime")?
        .block_on(future)
}

fn run_blocking_outside_tokio<F, T>(f: F) -> Result<T>
where
    F: FnOnce() -> Result<T> + Send,
    T: Send,
{
    if tokio::runtime::Handle::try_current().is_ok() {
        return std::thread::scope(|scope| {
            scope
                .spawn(f)
                .join()
                .map_err(|_| anyhow!("genskill blocking worker panicked"))?
        });
    }

    f()
}

fn parse_args(args: &str) -> Result<GepaOptions> {
    let mut opts = GepaOptions::default();
    let tokens: Vec<&str> = args.split_whitespace().collect();
    let mut index = 0;
    while index < tokens.len() {
        match tokens[index] {
            "--candidates" => {
                opts.n_candidates = tokens
                    .get(index + 1)
                    .and_then(|token| token.parse().ok())
                    .unwrap_or(3);
                index += 2;
            }
            "--rounds" => {
                opts.k_rounds = tokens
                    .get(index + 1)
                    .and_then(|token| token.parse().ok())
                    .unwrap_or(2);
                index += 2;
            }
            _ => index += 1,
        }
    }
    Ok(opts)
}

fn build_trace_from_state(state: &AppState) -> Result<ExecutionTrace> {
    let mut steps = Vec::new();
    for message in &state.transcript {
        match message.role {
            MessageRole::User => steps.push(TranscriptStep {
                role: "user".to_string(),
                text: message.text.clone(),
                tool_calls: Vec::new(),
                error: false,
            }),
            MessageRole::Assistant => steps.push(TranscriptStep {
                role: "assistant".to_string(),
                text: message.text.clone(),
                tool_calls: Vec::new(),
                error: false,
            }),
            MessageRole::ToolCall => steps.push(TranscriptStep {
                role: "assistant".to_string(),
                text: message.text.clone(),
                tool_calls: message.tool_id.iter().cloned().collect(),
                error: false,
            }),
            MessageRole::ToolResult => steps.push(TranscriptStep {
                role: "tool".to_string(),
                text: message.text.clone(),
                tool_calls: message.tool_id.iter().cloned().collect(),
                error: !message.success.unwrap_or(true),
            }),
            MessageRole::System => {}
        }
    }
    Ok(puffer_skill_evolution::extract_trace(&steps))
}

fn write_skill_to_disk(candidate: &SkillCandidate) -> Result<PathBuf> {
    let base = PathBuf::from("resources/skills");
    let mut name = candidate.frontmatter.name.clone();
    let mut counter = 2u32;
    while base.join(&name).exists() {
        name = format!("{}-v{}", candidate.frontmatter.name, counter);
        counter += 1;
    }
    let dir = base.join(&name);
    fs::create_dir_all(&dir)
        .with_context(|| format!("creating generated skill directory {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    let frontmatter = serde_yaml::to_string(&candidate.frontmatter)?;
    let content = format!("---\n{}---\n{}", frontmatter, candidate.body);
    fs::write(&path, content)
        .with_context(|| format!("writing generated skill {}", path.display()))?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_defaults() {
        let opts = parse_args("").unwrap();
        assert_eq!(opts.n_candidates, 3);
        assert_eq!(opts.k_rounds, 2);
    }

    #[test]
    fn parse_args_overrides() {
        let opts = parse_args("--candidates 5 --rounds 3").unwrap();
        assert_eq!(opts.n_candidates, 5);
        assert_eq!(opts.k_rounds, 3);
    }

    #[test]
    fn block_on_genskill_future_outside_runtime() {
        let value = block_on_genskill_future(async { Ok(7) }).unwrap();
        assert_eq!(value, 7);
    }

    #[test]
    fn block_on_genskill_future_inside_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let value = block_on_genskill_future(async { Ok(7) }).unwrap();
            assert_eq!(value, 7);
        });
    }

    #[test]
    fn run_blocking_outside_tokio_allows_nested_block_on_inside_runtime() {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .unwrap();

        runtime.block_on(async {
            let value = run_blocking_outside_tokio(|| {
                tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap()
                    .block_on(async { Ok(7) })
            })
            .unwrap();
            assert_eq!(value, 7);
        });
    }
}
