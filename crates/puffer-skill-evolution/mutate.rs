//! Generate mutated candidates from Pareto survivors.

use crate::{parse_skill_md, AgentRuntime, ExecutionTrace, RubricScores, SkillCandidate};
use anyhow::Result;

/// Builds a mutation prompt for one survivor and its weakest dimension.
pub trait MutatePromptBuilder: Send + Sync {
    /// Builds the mutation prompt for one survivor.
    fn build(&self, trace: &ExecutionTrace, survivor: &SkillCandidate, weakest: &str) -> String;
}

/// Default inline-template mutation prompt builder.
pub struct DefaultMutatePrompt;

impl MutatePromptBuilder for DefaultMutatePrompt {
    fn build(&self, trace: &ExecutionTrace, survivor: &SkillCandidate, weakest: &str) -> String {
        let trace_yaml = serde_yaml::to_string(trace).unwrap_or_default();
        format!(
            "You will refine a SKILL.md draft to improve its weakest dimension: {weakest}.\n\
             Preserve the strengths of the draft. Do not regress on other dimensions. Output\n\
             ONLY the revised SKILL.md (frontmatter and body), no commentary. Stay under 15000 bytes.\n\
             The revised skill must be conditional and verifier-first: preserve task/domain\n\
             triggers, required artifacts, schemas, tests, permissions, and success signals.\n\
             Keep the skill narrow: remove unrelated task families from broad catch-all drafts\n\
             and preserve the single clearest reusable workflow.\n\
             If the draft only restates a lightweight artifact contract, either add the\n\
             non-obvious recovery/verification method from the trace or narrow the skill until\n\
             it no longer competes with ordinary task instructions.\n\
             Add or keep non-interference guidance so future task prompts and verifiers override\n\
             the skill when they differ. Remove generic advice that would cause extra exploration.\n\
             If the trace shows a \"looks done but verifier failed\" lesson, preserve the exact\n\
             missing acceptance guard, such as running service state, report schema/CWE labels,\n\
             fallback artifact creation after denied shell commands, or both polyglot entrypoints.\n\n\
             ORIGINAL TRACE (yaml):\n{trace_yaml}\n\n\
             CURRENT DRAFT:\n---\nname: {name}\ndescription: {description}\n---\n{body}\n",
            weakest = weakest,
            trace_yaml = trace_yaml,
            name = survivor.frontmatter.name,
            description = survivor.frontmatter.description,
            body = survivor.body,
        )
    }
}

fn weakest_dimension(scores: &RubricScores) -> &'static str {
    let pairs = [
        ("novelty", scores.novelty),
        ("reproducibility", scores.reproducibility),
        ("structure", scores.structure),
        ("conciseness", scores.conciseness),
    ];
    pairs
        .iter()
        .min_by(|left, right| {
            left.1
                .partial_cmp(&right.1)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(name, _)| *name)
        .unwrap_or("structure")
}

/// Generates one mutated candidate per scored survivor.
///
/// Invalid mutation outputs are discarded. The original survivors are expected
/// to remain in the upstream candidate pool.
pub async fn mutate_survivors<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn MutatePromptBuilder,
    trace: &ExecutionTrace,
    survivors: &[SkillCandidate],
    max_size_bytes: usize,
) -> Result<Vec<SkillCandidate>> {
    let mut mutants = Vec::new();
    for (index, survivor) in survivors.iter().enumerate() {
        let Some(scores) = survivor.scores else {
            continue;
        };
        let weakest = weakest_dimension(&scores);
        let prompt = builder.build(trace, survivor, weakest);
        match runtime.invoke_agent(&prompt).await {
            Ok(raw) => match parse_skill_md(&raw) {
                Ok(candidate) if candidate.body.len() <= max_size_bytes => mutants.push(candidate),
                Ok(_) => tracing::warn!(index, "mutant exceeded size budget"),
                Err(error) => {
                    tracing::warn!(index, error = %error, "invalid mutant frontmatter");
                }
            },
            Err(error) => tracing::warn!(index, error = %error, "mutation runtime error"),
        }
    }
    Ok(mutants)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SkillFrontmatter, TraceEntry};
    use std::sync::Mutex;

    struct ScriptedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for ScriptedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn make_scored(scores: RubricScores) -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: "body".into(),
            scores: Some(scores),
        }
    }

    fn sample_trace() -> ExecutionTrace {
        ExecutionTrace {
            entries: vec![TraceEntry {
                summary: "x".into(),
                tool_calls: vec![],
                succeeded: true,
            }],
            task_summary: "x".into(),
        }
    }

    #[test]
    fn weakest_picks_lowest_dim() {
        let scores = RubricScores {
            novelty: 0.9,
            reproducibility: 0.3,
            structure: 0.7,
            conciseness: 0.8,
        };
        assert_eq!(weakest_dimension(&scores), "reproducibility");
    }

    #[tokio::test]
    async fn mutate_returns_valid_mutants() {
        let valid = "---\nname: foo\ndescription: bar\n---\nbody";
        let runtime = ScriptedRuntime(Mutex::new(vec![valid.into(), valid.into()]));
        let survivors = vec![
            make_scored(RubricScores {
                novelty: 0.5,
                reproducibility: 0.5,
                structure: 0.5,
                conciseness: 0.5,
            }),
            make_scored(RubricScores {
                novelty: 0.6,
                reproducibility: 0.6,
                structure: 0.6,
                conciseness: 0.6,
            }),
        ];
        let mutants = mutate_survivors(
            &runtime,
            &DefaultMutatePrompt,
            &sample_trace(),
            &survivors,
            15_000,
        )
        .await
        .unwrap();
        assert_eq!(mutants.len(), 2);
    }

    #[tokio::test]
    async fn mutate_discards_invalid() {
        let runtime = ScriptedRuntime(Mutex::new(vec!["garbage".into()]));
        let survivors = vec![make_scored(RubricScores {
            novelty: 0.5,
            reproducibility: 0.5,
            structure: 0.5,
            conciseness: 0.5,
        })];
        let mutants = mutate_survivors(
            &runtime,
            &DefaultMutatePrompt,
            &sample_trace(),
            &survivors,
            15_000,
        )
        .await
        .unwrap();
        assert!(mutants.is_empty());
    }

    #[test]
    fn default_mutation_prompt_preserves_non_interference() {
        let survivor = make_scored(RubricScores {
            novelty: 0.5,
            reproducibility: 0.5,
            structure: 0.5,
            conciseness: 0.5,
        });
        let prompt = DefaultMutatePrompt.build(&sample_trace(), &survivor, "reproducibility");

        assert!(prompt.contains("conditional and verifier-first"));
        assert!(prompt.contains("Keep the skill narrow"));
        assert!(prompt.contains("lightweight artifact contract"));
        assert!(prompt.contains("future task prompts and verifiers override"));
        assert!(prompt.contains("Remove generic advice"));
        assert!(prompt.contains("looks done but verifier failed"));
    }
}
