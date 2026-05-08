//! Aggregates per-replay metrics and renders a markdown report.

use crate::metrics::ReplayMetrics;
use crate::replay::Arm;
use std::collections::BTreeMap;
use std::fmt::Write as _;

/// Aggregated stats for one arm across all PRs.
#[derive(Debug, Clone)]
pub struct ArmSummary {
    pub passed: u64,
    pub total: u64,
    pub mean_tokens: f64,
    pub mean_tool_calls: f64,
    pub mean_duplicate: f64,
    pub mean_file_overlap: f64,
    pub mean_symbol_overlap: f64,
}

/// Per-PR metrics for all three arms.
pub type PrTriple = BTreeMap<Arm, ReplayMetrics>;

/// Aggregates metrics across all PRs for one arm.
pub fn aggregate(metrics_by_pr: &BTreeMap<String, PrTriple>, arm: Arm) -> ArmSummary {
    let mut s = ArmSummary {
        passed: 0,
        total: 0,
        mean_tokens: 0.0,
        mean_tool_calls: 0.0,
        mean_duplicate: 0.0,
        mean_file_overlap: 0.0,
        mean_symbol_overlap: 0.0,
    };
    for triple in metrics_by_pr.values() {
        if let Some(m) = triple.get(&arm) {
            s.total += 1;
            if m.passed {
                s.passed += 1;
                s.mean_tokens += m.total_tokens as f64;
                s.mean_tool_calls += m.tool_calls as f64;
                s.mean_duplicate += m.duplicate_score as f64;
                s.mean_file_overlap += m.file_overlap as f64;
                s.mean_symbol_overlap += m.symbol_overlap as f64;
            }
        }
    }
    let n = s.passed.max(1) as f64;
    s.mean_tokens /= n;
    s.mean_tool_calls /= n;
    s.mean_duplicate /= n;
    s.mean_file_overlap /= n;
    s.mean_symbol_overlap /= n;
    s
}

/// Renders the aggregated summary as markdown.
pub fn render_summary(run_date: &str, metrics_by_pr: &BTreeMap<String, PrTriple>) -> String {
    let no_skill = aggregate(metrics_by_pr, Arm::NoSkill);
    let direct = aggregate(metrics_by_pr, Arm::Direct);
    let gepa = aggregate(metrics_by_pr, Arm::Gepa);

    let mut out = String::new();
    let _ = writeln!(out, "# /genskill Evaluation — Ladybird PR Replay\n");
    let _ = writeln!(
        out,
        "Run date: {run_date}  \nPRs: {}\n",
        metrics_by_pr.len()
    );
    let _ = writeln!(out, "## Headline\n");
    let _ = writeln!(out, "| Metric | no-skill | direct | gepa |");
    let _ = writeln!(out, "|--------|----------|--------|------|");
    let _ = writeln!(
        out,
        "| Pass rate | {}/{} | {}/{} | {}/{} |",
        no_skill.passed, no_skill.total, direct.passed, direct.total, gepa.passed, gepa.total
    );
    let _ = writeln!(
        out,
        "| Mean tokens (passed) | {:.0} | {:.0} | {:.0} |",
        no_skill.mean_tokens, direct.mean_tokens, gepa.mean_tokens
    );
    let _ = writeln!(
        out,
        "| Mean tool calls | {:.1} | {:.1} | {:.1} |",
        no_skill.mean_tool_calls, direct.mean_tool_calls, gepa.mean_tool_calls
    );
    let _ = writeln!(
        out,
        "| Mean duplicate_score | {:.1} | {:.1} | {:.1} |",
        no_skill.mean_duplicate, direct.mean_duplicate, gepa.mean_duplicate
    );
    let _ = writeln!(
        out,
        "| Mean file_overlap | {:.2} | {:.2} | {:.2} |",
        no_skill.mean_file_overlap, direct.mean_file_overlap, gepa.mean_file_overlap
    );
    let _ = writeln!(
        out,
        "| Mean symbol_overlap | {:.2} | {:.2} | {:.2} |",
        no_skill.mean_symbol_overlap, direct.mean_symbol_overlap, gepa.mean_symbol_overlap
    );
    let _ = writeln!(out, "\n## Per-PR breakdown\n");
    for (pr, triple) in metrics_by_pr {
        let _ = writeln!(out, "### {pr}\n");
        let _ = writeln!(
            out,
            "| Arm | Outcome | Tokens | Tools | Dup | FileOvl | SymOvl |"
        );
        let _ = writeln!(
            out,
            "|-----|---------|--------|-------|-----|---------|--------|"
        );
        for arm in [Arm::NoSkill, Arm::Direct, Arm::Gepa] {
            if let Some(m) = triple.get(&arm) {
                let _ = writeln!(
                    out,
                    "| {:?} | {} | {} | {} | {} | {:.2} | {:.2} |",
                    arm,
                    if m.passed { "pass" } else { "fail" },
                    m.total_tokens,
                    m.tool_calls,
                    m.duplicate_score,
                    m.file_overlap,
                    m.symbol_overlap
                );
            }
        }
        let _ = writeln!(out);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aggregate_empty_is_zeroed() {
        let by_pr: BTreeMap<String, PrTriple> = BTreeMap::new();
        let s = aggregate(&by_pr, Arm::Gepa);
        assert_eq!(s.total, 0);
        assert_eq!(s.passed, 0);
    }
}
