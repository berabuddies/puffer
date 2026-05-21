//! Ladybird PR replay benchmark for /genskill.
//!
//! See spec at docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md.

#![deny(missing_docs)]

mod metrics;
mod pr_corpus;
mod replay;
mod report;
mod sandbox;
mod transcript;

use anyhow::Result;
use clap::{Parser, Subcommand};

/// CLI entry point.
#[derive(Parser)]
#[command(
    name = "puffer-genskill-eval",
    about = "Ladybird PR replay benchmark for /genskill"
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

/// Top-level subcommands.
#[derive(Subcommand)]
enum Cmd {
    /// Validate the on-disk corpus structure.
    Validate,
    /// Convert a puffer session JSONL transcript to flat markdown.
    TranscriptToMd {
        /// Input JSONL transcript path.
        #[arg(long = "in")]
        input: std::path::PathBuf,
        /// Output markdown path.
        #[arg(long = "out")]
        output: std::path::PathBuf,
    },
    /// Run a single replay: one PR, one arm.
    Replay {
        /// PR id (matches pr_corpus/<id>/).
        pr: String,
        /// Replay arm: no-skill | direct | gepa.
        arm: String,
        /// Run date directory under reports/. Defaults to today's UTC date.
        #[arg(long)]
        run_date: Option<String>,
    },
    /// Aggregate completed replays into a single report.
    Aggregate {
        /// Run date directory under reports/ (e.g., 2026-05-20).
        run_date: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.cmd {
        Cmd::Validate => {
            let entries = pr_corpus::load_corpus(std::path::Path::new(
                "benchmark/genskill/ladybird/pr_corpus",
            ))?;
            println!("OK: {} entries", entries.len());
            for e in &entries {
                println!("  {} ({}, {})", e.id, e.meta.area, e.meta.title);
            }
            Ok(())
        }
        Cmd::TranscriptToMd { input, output } => {
            transcript::transcript_to_md(&input, &output)?;
            println!("Wrote {}", output.display());
            Ok(())
        }
        Cmd::Replay { pr, arm, run_date } => {
            let arm = replay::Arm::parse(&arm)?;
            let entries = pr_corpus::load_corpus(std::path::Path::new(
                "benchmark/genskill/ladybird/pr_corpus",
            ))?;
            let entry = entries
                .iter()
                .find(|e| e.id == pr)
                .ok_or_else(|| anyhow::anyhow!("pr {pr} not in corpus"))?;
            let run_date =
                run_date.unwrap_or_else(|| chrono::Utc::now().format("%Y-%m-%d").to_string());
            let cfg = replay::ReplayConfig {
                corpus_entry: entry,
                arm,
                puffer_bin_host_path: env_nonempty("PUFFER_REPLAY_BIN")
                    .map(std::path::PathBuf::from)
                    .unwrap_or_else(default_replay_puffer_bin),
                agent_provider: env_nonempty("PUFFER_EVAL_PROVIDER")
                    .unwrap_or_else(|| "openai".to_string()),
                agent_model: env_nonempty("PUFFER_EVAL_MODEL")
                    .or_else(|| env_nonempty("PUFFER_MODEL"))
                    .unwrap_or_else(|| "gpt-5.4".to_string()),
                agent_effort: env_nonempty("PUFFER_EVAL_EFFORT")
                    .or_else(|| env_nonempty("PUFFER_EFFORT")),
                image: sandbox::DEFAULT_IMAGE.to_string(),
                wall_budget: std::time::Duration::from_secs(30 * 60),
                tool_budget: 50,
                token_budget: 250_000,
                run_date_dir: std::path::PathBuf::from(format!(
                    "benchmark/genskill/ladybird/reports/{run_date}"
                )),
            };
            let artifact = replay::run_one(cfg).await?;
            println!("{}", serde_json::to_string_pretty(&artifact)?);
            Ok(())
        }
        Cmd::Aggregate { run_date } => {
            let dir =
                std::path::PathBuf::from(format!("benchmark/genskill/ladybird/reports/{run_date}"));
            let entries = pr_corpus::load_corpus(std::path::Path::new(
                "benchmark/genskill/ladybird/pr_corpus",
            ))?;
            let mut by_pr: std::collections::BTreeMap<String, report::PrTriple> =
                std::collections::BTreeMap::new();
            for entry in &entries {
                let mut triple: report::PrTriple = std::collections::BTreeMap::new();
                for arm in [replay::Arm::NoSkill, replay::Arm::Direct, replay::Arm::Gepa] {
                    let path = dir.join(format!("{}-{:?}.json", entry.id, arm));
                    if !path.exists() {
                        continue;
                    }
                    let artifact: replay::ReplayArtifact =
                        serde_json::from_str(&std::fs::read_to_string(&path)?)?;
                    let reference_fix =
                        std::fs::read_to_string(entry.dir.join("reference_fix.patch"))
                            .unwrap_or_default();
                    triple.insert(arm, metrics::compute(&artifact, &reference_fix));
                }
                if !triple.is_empty() {
                    by_pr.insert(entry.id.clone(), triple);
                }
            }
            let md = report::render_summary(&run_date, &by_pr);
            let out_path = dir.join("summary.md");
            std::fs::write(&out_path, &md)?;
            println!("{md}");
            println!("\n(saved to {})", out_path.display());
            Ok(())
        }
    }
}

fn default_replay_puffer_bin() -> std::path::PathBuf {
    let linux_bin = std::path::PathBuf::from("benchmark/genskill/ladybird/.bin/puffer-linux");
    if linux_bin.exists() {
        linux_bin
    } else {
        std::path::PathBuf::from("target/release/puffer")
    }
}

fn env_nonempty(name: &str) -> Option<String> {
    let value = std::env::var(name).ok()?;
    let trimmed = value.trim().to_string();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed)
    }
}
