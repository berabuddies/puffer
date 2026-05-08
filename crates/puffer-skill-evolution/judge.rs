//! LLM-as-judge scoring of skill candidates.

use crate::{AgentRuntime, RubricScores, SkillCandidate};
use anyhow::{anyhow, Context, Result};
use serde::Deserialize;

/// Builds the prompt sent to the judge sub-agent.
pub trait JudgePromptBuilder: Send + Sync {
    /// Returns a complete prompt for one judging call.
    fn build(&self, candidate: &SkillCandidate) -> String;
}

/// Default judge prompt builder using an inline rubric template.
pub struct DefaultJudgePrompt;

impl JudgePromptBuilder for DefaultJudgePrompt {
    fn build(&self, candidate: &SkillCandidate) -> String {
        format!(
            "You are an LLM judge scoring a generated SKILL.md against four dimensions.\n\
             For each dimension, return a float in [0.0, 1.0]:\n\
             - novelty: captures non-obvious knowledge\n\
             - reproducibility: a fresh agent could reproduce the approach\n\
             - structure: proper sections (overview, when-to-use, pitfalls, checklist)\n\
             - conciseness: stays within budget without fluff\n\n\
             Reply ONLY with a JSON object:\n\
             {{\"novelty\":0.x,\"reproducibility\":0.x,\"structure\":0.x,\"conciseness\":0.x}}\n\n\
             SKILL FRONTMATTER:\nname: {name}\ndescription: {description}\n\n\
             SKILL BODY:\n{body}\n",
            name = candidate.frontmatter.name,
            description = candidate.frontmatter.description,
            body = candidate.body,
        )
    }
}

#[derive(Deserialize)]
struct JudgeReply {
    novelty: f32,
    reproducibility: f32,
    structure: f32,
    conciseness: f32,
}

/// Scores one candidate by invoking the runtime with the judge prompt.
///
/// Retries once on malformed JSON. Returns zero scores if both attempts fail
/// to parse, keeping the candidate in the pool but unlikely to survive.
pub async fn score_candidate<R: AgentRuntime + ?Sized>(
    runtime: &R,
    builder: &dyn JudgePromptBuilder,
    candidate: &SkillCandidate,
) -> Result<RubricScores> {
    let prompt = builder.build(candidate);
    let mut last_err = None;
    for _ in 0..2 {
        let raw = runtime.invoke_agent(&prompt).await.context("judge invocation")?;
        match parse_scores(&raw) {
            Ok(scores) => return Ok(scores),
            Err(error) => last_err = Some(error),
        }
    }
    tracing::warn!(?last_err, "judge produced malformed scores after retry");
    Ok(RubricScores {
        novelty: 0.0,
        reproducibility: 0.0,
        structure: 0.0,
        conciseness: 0.0,
    })
}

fn parse_scores(raw: &str) -> Result<RubricScores> {
    let start = raw
        .find('{')
        .ok_or_else(|| anyhow!("no JSON object in judge reply"))?;
    let end = raw
        .rfind('}')
        .ok_or_else(|| anyhow!("no closing brace in judge reply"))?;
    let json = &raw[start..=end];
    let reply: JudgeReply = serde_json::from_str(json).context("parsing judge JSON")?;
    Ok(RubricScores {
        novelty: clamp01(reply.novelty),
        reproducibility: clamp01(reply.reproducibility),
        structure: clamp01(reply.structure),
        conciseness: clamp01(reply.conciseness),
    })
}

fn clamp01(value: f32) -> f32 {
    value.clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SkillFrontmatter;
    use std::sync::Mutex;

    struct FixedRuntime(Mutex<Vec<String>>);

    #[async_trait::async_trait]
    impl AgentRuntime for FixedRuntime {
        async fn invoke_agent(&self, _prompt: &str) -> Result<String> {
            Ok(self.0.lock().unwrap().remove(0))
        }
    }

    fn dummy_candidate() -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: "body".into(),
            scores: None,
        }
    }

    #[tokio::test]
    async fn score_parses_clean_json() {
        let runtime = FixedRuntime(Mutex::new(vec![
            r#"{"novelty":0.8,"reproducibility":0.9,"structure":0.7,"conciseness":0.6}"#
                .to_string(),
        ]));
        let scores = score_candidate(&runtime, &DefaultJudgePrompt, &dummy_candidate())
            .await
            .unwrap();
        assert!((scores.novelty - 0.8).abs() < 1e-6);
        assert!((scores.reproducibility - 0.9).abs() < 1e-6);
    }

    #[tokio::test]
    async fn score_extracts_json_from_chatter() {
        let runtime = FixedRuntime(Mutex::new(vec![
            r#"Scores: {"novelty":0.5,"reproducibility":0.5,"structure":0.5,"conciseness":0.5} done."#
                .to_string(),
        ]));
        let scores = score_candidate(&runtime, &DefaultJudgePrompt, &dummy_candidate())
            .await
            .unwrap();
        assert!((scores.total() - 2.0).abs() < 1e-6);
    }

    #[tokio::test]
    async fn score_defaults_to_zero_after_two_bad_replies() {
        let runtime = FixedRuntime(Mutex::new(vec!["garbage".into(), "still garbage".into()]));
        let scores = score_candidate(&runtime, &DefaultJudgePrompt, &dummy_candidate())
            .await
            .unwrap();
        assert_eq!(scores.total(), 0.0);
    }

    #[tokio::test]
    async fn score_clamps_out_of_range() {
        let runtime = FixedRuntime(Mutex::new(vec![
            r#"{"novelty":1.5,"reproducibility":-0.3,"structure":0.5,"conciseness":0.5}"#
                .to_string(),
        ]));
        let scores = score_candidate(&runtime, &DefaultJudgePrompt, &dummy_candidate())
            .await
            .unwrap();
        assert_eq!(scores.novelty, 1.0);
        assert_eq!(scores.reproducibility, 0.0);
    }
}
