//! Experimental Burbot / Meta-PESA runtime.
//!
//! Burbot is intentionally separate from Puffer's conversational TUI. It uses
//! Puffer's provider/auth crates where an LLM probe is needed, while the core
//! runtime remains contract-driven and deterministic.

#![allow(dead_code)]

mod belief;
mod calibration;
mod cli;
mod contract;
#[cfg(feature = "egg-optimizer")]
mod egg_optimizer;
mod eval;
mod evolve;
mod executor;
mod failure;
mod graph;
mod graph_store;
mod ids;
mod llm;
mod plan_synthesis;
mod planner;
mod promotion;
mod puffer_tools;
mod rules;
mod runtime;
mod saturation;
mod scheduler;
mod semantics;
mod stats;
mod symbolic;
mod trace;
mod verification;

/// Runs the Burbot command-line interface.
pub fn run_cli() -> anyhow::Result<()> {
    cli::run()
}
