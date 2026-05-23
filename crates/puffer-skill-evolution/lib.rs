//! GEPA-style skill generation from conversation traces.
//!
//! Implements the core loop for `/genskill`: multi-candidate generation,
//! LLM-as-judge scoring, Pareto selection, and mutation rounds. The
//! `AgentRuntime` trait abstracts LLM dispatch so tests can mock calls.

#![deny(missing_docs)]

mod generate;
mod judge;
mod mutate;
mod pareto;
mod parse;
mod trace;

pub use generate::{generate_candidates, DefaultGeneratePrompt, GeneratePromptBuilder};
pub use judge::{score_candidate, DefaultJudgePrompt, JudgePromptBuilder};
pub use mutate::{mutate_survivors, DefaultMutatePrompt, MutatePromptBuilder};
pub use pareto::pareto_frontier;
pub use parse::parse_skill_md;
pub use trace::{extract_trace, TranscriptStep};

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// Per-skill rubric scores produced by the LLM-as-judge pass.
///
/// Each field is in the range `[0.0, 1.0]`. A skill is Pareto-dominated
/// by another iff the other's score is `>=` on every dimension and `>` on
/// at least one.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct RubricScores {
    /// Captures non-obvious knowledge from the trace (0.0-1.0).
    pub novelty: f32,
    /// A fresh agent reading only this skill could reproduce the approach (0.0-1.0).
    pub reproducibility: f32,
    /// Has proper sections (Overview, When to Use, Pitfalls, Checklist) (0.0-1.0).
    pub structure: f32,
    /// Concise and within size budget (0.0-1.0).
    pub conciseness: f32,
}

impl RubricScores {
    /// Returns true if `self` Pareto-dominates `other`.
    pub fn dominates(&self, other: &Self) -> bool {
        let dims = [
            (self.novelty, other.novelty),
            (self.reproducibility, other.reproducibility),
            (self.structure, other.structure),
            (self.conciseness, other.conciseness),
        ];
        let all_ge = dims.iter().all(|(score, other_score)| score >= other_score);
        let any_gt = dims.iter().any(|(score, other_score)| score > other_score);
        all_ge && any_gt
    }

    /// Returns the sum of all four dimensions for tie-breaking.
    pub fn total(&self) -> f32 {
        self.novelty + self.reproducibility + self.structure + self.conciseness
    }
}

/// Configuration for one `/genskill` invocation.
#[derive(Debug, Clone)]
pub struct GepaOptions {
    /// Number of candidates per round.
    pub n_candidates: usize,
    /// Number of evolution rounds after the initial generation pass.
    pub k_rounds: usize,
    /// Hard size budget for a candidate skill body.
    pub max_size_bytes: usize,
}

impl Default for GepaOptions {
    fn default() -> Self {
        Self {
            n_candidates: 3,
            k_rounds: 2,
            max_size_bytes: 15_000,
        }
    }
}

/// Structured execution trace extracted from a session transcript.
///
/// The trace is what the generation sub-agent reads as evidence of a
/// non-trivial task. It captures tool calls, outcomes, failures, and
/// the rough shape of the conversation without including raw provider
/// system prompts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionTrace {
    /// One entry per significant turn.
    pub entries: Vec<TraceEntry>,
    /// Human-readable summary of the task being attempted.
    pub task_summary: String,
}

/// One step of an `ExecutionTrace`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceEntry {
    /// Brief description of what happened in this step.
    pub summary: String,
    /// Names of tools called in this step, if any.
    pub tool_calls: Vec<String>,
    /// Whether this step succeeded according to a best-effort heuristic.
    pub succeeded: bool,
}

/// Frontmatter of a generated skill file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillFrontmatter {
    /// Lowercase-hyphen skill name, up to 64 chars.
    pub name: String,
    /// One-line use-case trigger description, up to 1024 chars.
    pub description: String,
}

/// A candidate skill produced by the generation or mutation step.
#[derive(Debug, Clone)]
pub struct SkillCandidate {
    /// Parsed frontmatter.
    pub frontmatter: SkillFrontmatter,
    /// Body text after the closing frontmatter delimiter.
    pub body: String,
    /// Scores from the judge pass; `None` until scored.
    pub scores: Option<RubricScores>,
}

impl SkillCandidate {
    /// Returns the approximate serialized byte length of frontmatter plus body.
    pub fn size_bytes(&self) -> usize {
        self.body.len() + self.frontmatter.name.len() + self.frontmatter.description.len() + 64
    }
}

/// Runs the full GEPA loop and returns the Pareto-best candidate.
///
/// Round 0 generates candidates from the trace. Each round scores unscored
/// candidates, computes the Pareto frontier, and mutates survivors until the
/// configured round budget is exhausted. The final selection prefers highest
/// reproducibility and then highest total score.
pub async fn run_gepa<R: AgentRuntime + ?Sized>(
    runtime: &R,
    trace: &ExecutionTrace,
    opts: &GepaOptions,
    generate_builder: &dyn generate::GeneratePromptBuilder,
    judge_builder: &dyn judge::JudgePromptBuilder,
    mutate_builder: &dyn mutate::MutatePromptBuilder,
) -> Result<SkillCandidate> {
    let mut pool = generate::generate_candidates(
        runtime,
        generate_builder,
        trace,
        opts.n_candidates,
        opts.max_size_bytes,
    )
    .await?;

    let mut frontier_indices = Vec::new();
    for round in 0..=opts.k_rounds {
        for candidate in &mut pool {
            if candidate.scores.is_none() {
                let scores = judge::score_candidate(runtime, judge_builder, candidate).await?;
                candidate.scores = Some(scores);
            }
        }
        frontier_indices = pareto::pareto_frontier(&pool);
        tracing::info!(
            round,
            pool = pool.len(),
            frontier = frontier_indices.len(),
            "GEPA round complete"
        );

        if round < opts.k_rounds {
            let survivors: Vec<SkillCandidate> = frontier_indices
                .iter()
                .map(|&index| pool[index].clone())
                .collect();
            let mutants = mutate::mutate_survivors(
                runtime,
                mutate_builder,
                trace,
                &survivors,
                opts.max_size_bytes,
            )
            .await?;
            pool.extend(mutants);
        }
    }

    select_best(&pool, &frontier_indices)
}

fn select_best(pool: &[SkillCandidate], frontier: &[usize]) -> Result<SkillCandidate> {
    let mut best: Option<&SkillCandidate> = None;
    for &index in frontier {
        let candidate = &pool[index];
        let Some(scores) = candidate.scores else {
            continue;
        };
        match best {
            None => best = Some(candidate),
            Some(current) => {
                let current_scores = current.scores.unwrap();
                let prefer = scores.reproducibility > current_scores.reproducibility
                    || (scores.reproducibility == current_scores.reproducibility
                        && scores.total() > current_scores.total());
                if prefer {
                    best = Some(candidate);
                }
            }
        }
    }
    best.cloned()
        .ok_or_else(|| anyhow::anyhow!("frontier empty after GEPA loop"))
}

/// Abstraction over LLM dispatch so generation, judging, and mutation are testable.
///
/// In production, this is implemented by a thin wrapper around puffer-core's
/// live agent dispatch. Tests provide canned-response implementations.
#[async_trait::async_trait]
pub trait AgentRuntime: Send + Sync {
    /// Invokes a sub-agent with the given prompt and returns its full text response.
    ///
    /// Errors if the sub-agent invocation fails or times out. The response content
    /// is not validated by this trait; callers parse it.
    async fn invoke_agent(&self, prompt: &str) -> anyhow::Result<String>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    struct CannedRuntime {
        responses: Mutex<Vec<String>>,
    }

    struct OrchestratedRuntime {
        responses: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl AgentRuntime for CannedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> anyhow::Result<String> {
            let mut responses = self.responses.lock().unwrap();
            Ok(responses.remove(0))
        }
    }

    #[async_trait::async_trait]
    impl AgentRuntime for OrchestratedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> anyhow::Result<String> {
            Ok(self.responses.lock().unwrap().remove(0))
        }
    }

    #[test]
    fn rubric_scores_pareto_dominance() {
        let a = RubricScores {
            novelty: 0.8,
            reproducibility: 0.9,
            structure: 0.7,
            conciseness: 0.6,
        };
        let b = RubricScores {
            novelty: 0.7,
            reproducibility: 0.8,
            structure: 0.6,
            conciseness: 0.5,
        };
        assert!(a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn rubric_scores_pareto_incomparable() {
        let a = RubricScores {
            novelty: 0.9,
            reproducibility: 0.5,
            structure: 0.7,
            conciseness: 0.6,
        };
        let b = RubricScores {
            novelty: 0.5,
            reproducibility: 0.9,
            structure: 0.7,
            conciseness: 0.6,
        };
        assert!(!a.dominates(&b));
        assert!(!b.dominates(&a));
    }

    #[test]
    fn rubric_scores_total() {
        let scores = RubricScores {
            novelty: 0.5,
            reproducibility: 0.5,
            structure: 0.5,
            conciseness: 0.5,
        };
        assert!((scores.total() - 2.0).abs() < 1e-6);
    }

    #[test]
    fn gepa_options_defaults() {
        let opts = GepaOptions::default();
        assert_eq!(opts.n_candidates, 3);
        assert_eq!(opts.k_rounds, 2);
        assert_eq!(opts.max_size_bytes, 15_000);
    }

    #[tokio::test]
    async fn mock_runtime_returns_canned_response() {
        let runtime = CannedRuntime {
            responses: Mutex::new(vec!["hello".to_string()]),
        };
        let out = runtime.invoke_agent("ignored").await.unwrap();
        assert_eq!(out, "hello");
    }

    #[tokio::test]
    async fn run_gepa_returns_best_from_frontier() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let runtime = OrchestratedRuntime {
            responses: Mutex::new(vec![
                valid.into(),
                valid.into(),
                valid.into(),
                r#"{"novelty":0.9,"reproducibility":0.9,"structure":0.9,"conciseness":0.9}"#.into(),
                r#"{"novelty":0.5,"reproducibility":0.5,"structure":0.5,"conciseness":0.5}"#.into(),
                r#"{"novelty":0.4,"reproducibility":0.4,"structure":0.4,"conciseness":0.4}"#.into(),
                valid.into(),
                r#"{"novelty":0.95,"reproducibility":0.95,"structure":0.95,"conciseness":0.95}"#
                    .into(),
            ]),
        };
        let trace = ExecutionTrace {
            entries: vec![TraceEntry {
                summary: "s".into(),
                tool_calls: vec![],
                succeeded: true,
            }],
            task_summary: "t".into(),
        };
        let opts = GepaOptions {
            n_candidates: 3,
            k_rounds: 1,
            max_size_bytes: 15_000,
        };
        let best = run_gepa(
            &runtime,
            &trace,
            &opts,
            &DefaultGeneratePrompt,
            &DefaultJudgePrompt,
            &DefaultMutatePrompt,
        )
        .await
        .unwrap();
        let scores = best.scores.unwrap();
        assert!((scores.total() - 3.8).abs() < 1e-3);
    }
}
