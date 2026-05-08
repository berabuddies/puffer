//! One-replay execution: spawn sandbox, run puffer, capture artifact.

use crate::pr_corpus::CorpusEntry;
use crate::sandbox::{Sandbox, DEFAULT_IMAGE};
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tokio::time::Duration;

/// A replay arm: which skill (if any) was loaded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, PartialOrd, Ord)]
#[serde(rename_all = "kebab-case")]
pub enum Arm {
    /// No skill loaded (baseline).
    NoSkill,
    /// Direct-prompt-generated skill loaded.
    Direct,
    /// /genskill GEPA-generated skill loaded.
    Gepa,
}

impl Arm {
    /// Parses a CLI string into an Arm.
    pub fn parse(s: &str) -> Result<Arm> {
        match s {
            "no-skill" => Ok(Arm::NoSkill),
            "direct" => Ok(Arm::Direct),
            "gepa" => Ok(Arm::Gepa),
            _ => Err(anyhow!("unknown arm {s}; expected no-skill | direct | gepa")),
        }
    }
}

/// How a replay terminated.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Outcome {
    /// Target test passed.
    Pass,
    /// Test still failed after agent's claim of completion.
    WrongFix,
    /// Agent gave up.
    GaveUp,
    /// Wall-clock budget exceeded.
    WallTimeout,
    /// Tool-call cap exceeded.
    ToolBudget,
    /// Token budget exceeded.
    TokenBudget,
}

/// Token usage breakdown.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Tokens {
    pub input: u64,
    pub output: u64,
    pub tool_results: u64,
    pub total: u64,
}

/// One tool call recorded during the replay.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub name: String,
    pub input: serde_json::Value,
    pub output_size: u64,
    pub ts: String,
}

/// Outcome of running the target test.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestOutcome {
    pub command: String,
    pub exit_code: i32,
    pub stdout_tail: String,
}

/// Full replay artifact stored at reports/<run_date>/<pr>-<arm>.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReplayArtifact {
    pub pr: String,
    pub arm: Arm,
    pub outcome: Outcome,
    pub wall_seconds: u64,
    pub tool_calls: u64,
    pub tokens: Tokens,
    pub tool_call_log: Vec<ToolCall>,
    pub final_diff: String,
    pub test_outcome: Option<TestOutcome>,
}

/// Configuration for one replay.
pub struct ReplayConfig<'a> {
    pub corpus_entry: &'a CorpusEntry,
    pub arm: Arm,
    pub puffer_bin_host_path: PathBuf,
    pub image: String,
    pub wall_budget: Duration,
    pub tool_budget: u64,
    pub token_budget: u64,
    pub run_date_dir: PathBuf,
}

/// Runs one replay end-to-end. Writes the artifact JSON and returns it.
pub async fn run_one(cfg: ReplayConfig<'_>) -> Result<ReplayArtifact> {
    let started = std::time::Instant::now();
    let test_files_dir = cfg.corpus_entry.dir.join("tests");
    let sandbox = Sandbox::start(
        &cfg.image,
        &cfg.puffer_bin_host_path,
        &cfg.corpus_entry.meta.base_commit,
        &test_files_dir,
    )
    .await
    .context("starting sandbox")?;

    let test_filter = test_filter_for(cfg.corpus_entry);
    let pre_check = sandbox
        .exec(&["bash", "-c", &format!("ladybird-test --filter={test_filter}; echo 0")])
        .await
        .ok();
    tracing::info!(?pre_check, "pre-replay test status");

    let mut puffer_args = vec![
        "non-interactive".to_string(),
        "--user-message".to_string(),
        cfg.corpus_entry.meta.task_prompt.clone(),
        "--max-tool-calls".to_string(),
        cfg.tool_budget.to_string(),
        "--max-tokens".to_string(),
        cfg.token_budget.to_string(),
        "--emit-artifact".to_string(),
        "/tmp/replay-artifact.json".to_string(),
    ];
    if let Some(skill_path) = skill_path_for(&cfg) {
        puffer_args.push("--load-skill".to_string());
        puffer_args.push(skill_path);
    }

    let exec_args: Vec<&str> = std::iter::once("puffer")
        .chain(puffer_args.iter().map(String::as_str))
        .collect();

    let exec_result = tokio::time::timeout(cfg.wall_budget, sandbox.exec(&exec_args)).await;
    let outcome_kind = match exec_result {
        Err(_) => Outcome::WallTimeout,
        Ok(Err(e)) => {
            tracing::warn!(error = %e, "puffer exec failed");
            Outcome::GaveUp
        }
        Ok(Ok(_)) => Outcome::Pass,
    };

    let pulled = sandbox.exec(&["bash", "-c", "cat /tmp/replay-artifact.json"]).await;
    let mut artifact = if let Ok((stdout, _)) = pulled {
        serde_json::from_str::<ReplayArtifact>(&stdout).unwrap_or_else(|_| empty_artifact(&cfg))
    } else {
        empty_artifact(&cfg)
    };

    if matches!(outcome_kind, Outcome::WallTimeout) {
        artifact.outcome = Outcome::WallTimeout;
    }

    let test_run = sandbox
        .exec(&["bash", "-c", &format!("ladybird-test --filter={test_filter}; echo EXIT=0")])
        .await;
    if let Ok((stdout, _)) = test_run {
        let exit_code = parse_exit_code(&stdout).unwrap_or(-1);
        artifact.test_outcome = Some(TestOutcome {
            command: format!("ladybird-test --filter={test_filter}"),
            exit_code,
            stdout_tail: tail(&stdout, 4_000),
        });
        if exit_code == 0 && !matches!(artifact.outcome, Outcome::WallTimeout) {
            artifact.outcome = Outcome::Pass;
        } else if exit_code != 0 && matches!(artifact.outcome, Outcome::Pass) {
            artifact.outcome = Outcome::WrongFix;
        }
    }

    artifact.wall_seconds = started.elapsed().as_secs();

    std::fs::create_dir_all(&cfg.run_date_dir)
        .with_context(|| format!("creating {}", cfg.run_date_dir.display()))?;
    let artifact_path = cfg.run_date_dir
        .join(format!("{}-{:?}.json", cfg.corpus_entry.id, cfg.arm));
    std::fs::write(&artifact_path, serde_json::to_string_pretty(&artifact)?)?;
    tracing::info!(path = %artifact_path.display(), "wrote replay artifact");

    Ok(artifact)
}

fn empty_artifact(cfg: &ReplayConfig<'_>) -> ReplayArtifact {
    ReplayArtifact {
        pr: cfg.corpus_entry.id.clone(),
        arm: cfg.arm,
        outcome: Outcome::GaveUp,
        wall_seconds: 0,
        tool_calls: 0,
        tokens: Tokens::default(),
        tool_call_log: Vec::new(),
        final_diff: String::new(),
        test_outcome: None,
    }
}

fn skill_path_for(cfg: &ReplayConfig<'_>) -> Option<String> {
    match cfg.arm {
        Arm::NoSkill => None,
        Arm::Direct => Some(format!(
            "/host/{}/skills/direct/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
        Arm::Gepa => Some(format!(
            "/host/{}/skills/gepa/SKILL.md",
            cfg.corpus_entry.dir.display()
        )),
    }
}

fn test_filter_for(entry: &CorpusEntry) -> String {
    entry
        .meta
        .files_changed
        .iter()
        .find(|p| p.starts_with("Tests/"))
        .cloned()
        .unwrap_or_else(|| entry.id.clone())
}

fn parse_exit_code(s: &str) -> Option<i32> {
    s.lines()
        .rev()
        .find_map(|l| l.strip_prefix("EXIT=").and_then(|n| n.parse().ok()))
}

fn tail(s: &str, n: usize) -> String {
    if s.len() <= n { s.to_string() } else { s[s.len() - n..].to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arm_parses() {
        assert_eq!(Arm::parse("no-skill").unwrap(), Arm::NoSkill);
        assert_eq!(Arm::parse("direct").unwrap(), Arm::Direct);
        assert_eq!(Arm::parse("gepa").unwrap(), Arm::Gepa);
        assert!(Arm::parse("garbage").is_err());
    }

    #[test]
    fn parse_exit_extracts() {
        assert_eq!(parse_exit_code("some\nEXIT=0\n"), Some(0));
    }

    #[test]
    fn parse_exit_finds_last() {
        assert_eq!(parse_exit_code("EXIT=1\nlater\nEXIT=0\n"), Some(0));
    }

    #[test]
    fn tail_short_unchanged() {
        assert_eq!(tail("abc", 10), "abc");
    }

    #[test]
    fn tail_truncates() {
        assert_eq!(tail(&"x".repeat(50), 10).len(), 10);
    }
}
