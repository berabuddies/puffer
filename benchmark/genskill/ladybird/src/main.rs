//! Ladybird PR replay benchmark for /genskill.
//!
//! See spec at docs/superpowers/specs/2026-05-07-genskill-eval-ladybird.md.

#![deny(missing_docs)]

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
    /// Run a single replay: one PR, one arm.
    Replay {
        /// PR id (matches pr_corpus/<id>/).
        pr: String,
        /// Replay arm: no-skill | direct | gepa.
        arm: String,
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
            println!("Corpus validation not yet implemented");
            Ok(())
        }
        Cmd::Replay { pr, arm } => {
            println!("Replay {pr} {arm} not yet implemented");
            Ok(())
        }
        Cmd::Aggregate { run_date } => {
            println!("Aggregate {run_date} not yet implemented");
            Ok(())
        }
    }
}
