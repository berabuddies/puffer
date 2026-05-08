//! Non-dominated Pareto frontier selection for scored candidates.

use crate::SkillCandidate;

/// Returns the indices of candidates on the non-dominated frontier.
///
/// A candidate is on the frontier iff no other candidate dominates it.
/// Candidates with no scores are excluded. If no candidates have scores,
/// returns an empty vector.
pub fn pareto_frontier(candidates: &[SkillCandidate]) -> Vec<usize> {
    let mut frontier = Vec::new();
    for (index, candidate) in candidates.iter().enumerate() {
        let Some(scores) = candidate.scores else {
            continue;
        };
        let dominated = candidates.iter().enumerate().any(|(other_index, other)| {
            index != other_index
                && other
                    .scores
                    .is_some_and(|other_scores| other_scores.dominates(&scores))
        });
        if !dominated {
            frontier.push(index);
        }
    }
    frontier
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{RubricScores, SkillFrontmatter};

    fn make_candidate(scores: Option<RubricScores>) -> SkillCandidate {
        SkillCandidate {
            frontmatter: SkillFrontmatter {
                name: "test".into(),
                description: "test".into(),
            },
            body: String::new(),
            scores,
        }
    }

    #[test]
    fn frontier_excludes_unscored() {
        let candidates = vec![make_candidate(None), make_candidate(None)];
        assert!(pareto_frontier(&candidates).is_empty());
    }

    #[test]
    fn frontier_keeps_dominator_only() {
        let dominator = RubricScores {
            novelty: 0.9,
            reproducibility: 0.9,
            structure: 0.9,
            conciseness: 0.9,
        };
        let weak = RubricScores {
            novelty: 0.5,
            reproducibility: 0.5,
            structure: 0.5,
            conciseness: 0.5,
        };
        let candidates = vec![make_candidate(Some(dominator)), make_candidate(Some(weak))];
        assert_eq!(pareto_frontier(&candidates), vec![0]);
    }

    #[test]
    fn frontier_keeps_incomparable() {
        let first = RubricScores {
            novelty: 0.9,
            reproducibility: 0.5,
            structure: 0.7,
            conciseness: 0.7,
        };
        let second = RubricScores {
            novelty: 0.5,
            reproducibility: 0.9,
            structure: 0.7,
            conciseness: 0.7,
        };
        let candidates = vec![make_candidate(Some(first)), make_candidate(Some(second))];
        let frontier = pareto_frontier(&candidates);
        assert_eq!(frontier.len(), 2);
        assert!(frontier.contains(&0));
        assert!(frontier.contains(&1));
    }
}
