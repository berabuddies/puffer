use crate::graph::{ActionScores, PlanGraph};
use crate::ids::NodeId;
use serde::Serialize;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SchedulerWeights {
    pub(crate) expected_progress: f64,
    pub(crate) success_probability: f64,
    pub(crate) success_uncertainty_penalty: f64,
    pub(crate) information_gain: f64,
    pub(crate) verification_value: f64,
    pub(crate) reversibility_bonus: f64,
    pub(crate) risk_penalty: f64,
    pub(crate) cost_penalty: f64,
    pub(crate) latency_penalty: f64,
    pub(crate) uncertainty_penalty: f64,
    pub(crate) repeated_failure_penalty: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ActionEstimate {
    pub(crate) expected_progress: f64,
    pub(crate) success_probability: f64,
    pub(crate) success_uncertainty: f64,
    pub(crate) information_gain: f64,
    pub(crate) verification_value: f64,
    pub(crate) reversibility_bonus: f64,
    pub(crate) risk_penalty: f64,
    pub(crate) cost_penalty: f64,
    pub(crate) latency_penalty: f64,
    pub(crate) uncertainty_penalty: f64,
    pub(crate) repeated_failure_penalty: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct ScoreBreakdown {
    pub(crate) expected_progress: f64,
    pub(crate) success_probability: f64,
    pub(crate) success_uncertainty_penalty: f64,
    pub(crate) information_gain: f64,
    pub(crate) verification_value: f64,
    pub(crate) reversibility_bonus: f64,
    pub(crate) risk_penalty: f64,
    pub(crate) cost_penalty: f64,
    pub(crate) latency_penalty: f64,
    pub(crate) uncertainty_penalty: f64,
    pub(crate) repeated_failure_penalty: f64,
    pub(crate) total: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SelectedAction {
    pub(crate) node_id: NodeId,
    pub(crate) breakdown: ScoreBreakdown,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct Scheduler {
    pub(crate) weights: SchedulerWeights,
}

impl Default for SchedulerWeights {
    fn default() -> Self {
        Self {
            expected_progress: 1.0,
            success_probability: 0.6,
            success_uncertainty_penalty: 0.35,
            information_gain: 0.45,
            verification_value: 0.4,
            reversibility_bonus: 0.25,
            risk_penalty: 1.25,
            cost_penalty: 0.35,
            latency_penalty: 0.2,
            uncertainty_penalty: 0.5,
            repeated_failure_penalty: 1.0,
        }
    }
}

impl ActionEstimate {
    /// Converts graph scores into scheduler-ready calibrated action estimates.
    pub(crate) fn from_scores(scores: &ActionScores) -> Self {
        let success_probability = estimate_success_probability(scores);
        let success_uncertainty = estimate_success_uncertainty(scores);
        Self {
            expected_progress: scores.expected_progress,
            success_probability,
            success_uncertainty,
            information_gain: scores.information_gain,
            verification_value: scores.verification_value,
            reversibility_bonus: scores.reversibility_bonus,
            risk_penalty: scores.risk_penalty,
            cost_penalty: scores.cost_penalty,
            latency_penalty: scores.latency_penalty,
            uncertainty_penalty: scores.uncertainty_penalty,
            repeated_failure_penalty: scores.repeated_failure_penalty,
        }
    }
}

impl Scheduler {
    /// Chooses the highest-scored executable action from the plan graph.
    pub(crate) fn choose_next_action(&self, graph: &PlanGraph) -> Option<SelectedAction> {
        choose_next_action(graph, &self.weights)
    }
}

/// Chooses the highest-scored executable action with explicit scheduler weights.
pub(crate) fn choose_next_action(
    graph: &PlanGraph,
    weights: &SchedulerWeights,
) -> Option<SelectedAction> {
    scored_executable_actions(graph, weights)
        .into_iter()
        .max_by(|left, right| {
            left.breakdown
                .total
                .partial_cmp(&right.breakdown.total)
                .unwrap_or(std::cmp::Ordering::Equal)
        })
}

fn scored_executable_actions(graph: &PlanGraph, weights: &SchedulerWeights) -> Vec<SelectedAction> {
    graph
        .executable_frontier_actions()
        .into_iter()
        .filter_map(|node_id| {
            let node = graph.node(node_id).ok()?;
            let estimate = ActionEstimate::from_scores(&node.scores);
            let breakdown = score_action(&estimate, weights);
            Some(SelectedAction { node_id, breakdown })
        })
        .collect()
}

/// Scores an action estimate and returns a term-by-term breakdown.
pub(crate) fn score_action(
    estimate: &ActionEstimate,
    weights: &SchedulerWeights,
) -> ScoreBreakdown {
    let expected_progress = weights.expected_progress * estimate.expected_progress;
    let success_probability = weights.success_probability * estimate.success_probability;
    let success_uncertainty_penalty =
        weights.success_uncertainty_penalty * estimate.success_uncertainty;
    let information_gain = weights.information_gain * estimate.information_gain;
    let verification_value = weights.verification_value * estimate.verification_value;
    let reversibility_bonus = weights.reversibility_bonus * estimate.reversibility_bonus;
    let risk_penalty = weights.risk_penalty * estimate.risk_penalty;
    let cost_penalty = weights.cost_penalty * estimate.cost_penalty;
    let latency_penalty = weights.latency_penalty * estimate.latency_penalty;
    let uncertainty_penalty = weights.uncertainty_penalty * estimate.uncertainty_penalty;
    let repeated_failure_penalty =
        weights.repeated_failure_penalty * estimate.repeated_failure_penalty;
    let total = expected_progress
        + success_probability
        + information_gain
        + verification_value
        + reversibility_bonus
        - success_uncertainty_penalty
        - risk_penalty
        - cost_penalty
        - latency_penalty
        - uncertainty_penalty
        - repeated_failure_penalty;

    ScoreBreakdown {
        expected_progress,
        success_probability,
        success_uncertainty_penalty,
        information_gain,
        verification_value,
        reversibility_bonus,
        risk_penalty,
        cost_penalty,
        latency_penalty,
        uncertainty_penalty,
        repeated_failure_penalty,
        total,
    }
}

fn estimate_success_probability(scores: &ActionScores) -> f64 {
    let positive_evidence = 0.65 * scores.expected_progress.max(0.0)
        + 0.25 * scores.information_gain.max(0.0)
        + 0.25 * scores.verification_value.max(0.0)
        + 0.15 * scores.reversibility_bonus.max(0.0);
    let negative_evidence = 0.65 * scores.risk_penalty.max(0.0)
        + 0.45 * scores.uncertainty_penalty.max(0.0)
        + 0.95 * scores.repeated_failure_penalty.max(0.0)
        + 0.25 * scores.cost_penalty.max(0.0)
        + 0.2 * scores.latency_penalty.max(0.0);
    logistic(positive_evidence - negative_evidence)
}

fn estimate_success_uncertainty(scores: &ActionScores) -> f64 {
    let uncertainty = scores.uncertainty_penalty.max(0.0);
    (uncertainty / (1.0 + uncertainty)).clamp(0.0, 1.0)
}

fn logistic(value: f64) -> f64 {
    if value >= 0.0 {
        let exp = (-value).exp();
        1.0 / (1.0 + exp)
    } else {
        let exp = value.exp();
        exp / (1.0 + exp)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{PlanEdgeKind, PlanNode, PlanNodeKind, PlanStatus};
    use serde_json::json;

    fn action_node(label: &str, scores: ActionScores) -> PlanNode {
        PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Action,
            status: PlanStatus::Open,
            label: label.to_string(),
            payload: json!({}),
            action_ref: None,
            scores,
        }
    }

    fn scores(expected_progress: f64) -> ActionScores {
        ActionScores {
            expected_progress,
            information_gain: 0.0,
            verification_value: 0.0,
            reversibility_bonus: 0.0,
            cache_reuse_bonus: 0.0,
            risk_penalty: 0.0,
            cost_penalty: 0.0,
            latency_penalty: 0.0,
            uncertainty_penalty: 0.0,
            repeated_failure_penalty: 0.0,
        }
    }

    #[test]
    fn score_breakdown_explains_each_weighted_term() {
        let estimate = ActionEstimate {
            expected_progress: 2.0,
            success_probability: 0.75,
            success_uncertainty: 0.7,
            information_gain: 0.5,
            verification_value: 0.25,
            reversibility_bonus: 0.1,
            risk_penalty: 0.2,
            cost_penalty: 0.3,
            latency_penalty: 0.4,
            uncertainty_penalty: 0.5,
            repeated_failure_penalty: 0.6,
        };
        let weights = SchedulerWeights {
            expected_progress: 1.0,
            success_probability: 2.0,
            success_uncertainty_penalty: 11.0,
            information_gain: 3.0,
            verification_value: 4.0,
            reversibility_bonus: 5.0,
            risk_penalty: 6.0,
            cost_penalty: 7.0,
            latency_penalty: 8.0,
            uncertainty_penalty: 9.0,
            repeated_failure_penalty: 10.0,
        };

        let breakdown = score_action(&estimate, &weights);

        assert_close(breakdown.expected_progress, 2.0);
        assert_close(breakdown.success_probability, 1.5);
        assert_close(breakdown.success_uncertainty_penalty, 7.7);
        assert_close(breakdown.information_gain, 1.5);
        assert_close(breakdown.verification_value, 1.0);
        assert_close(breakdown.reversibility_bonus, 0.5);
        assert_close(breakdown.risk_penalty, 1.2);
        assert_close(breakdown.cost_penalty, 2.1);
        assert_close(breakdown.latency_penalty, 3.2);
        assert_close(breakdown.uncertainty_penalty, 4.5);
        assert_close(breakdown.repeated_failure_penalty, 6.0);
        assert_close(breakdown.total, -18.2);
    }

    #[test]
    fn chooser_selects_highest_scored_open_action() {
        let mut graph = PlanGraph::from_goal("ship".to_string());
        let low = graph.add_node(action_node("low", scores(0.2)));
        let high = graph.add_node(action_node("high", scores(1.0)));
        graph.node_mut(low).unwrap().status = PlanStatus::Open;

        let selected = choose_next_action(&graph, &SchedulerWeights::default()).unwrap();

        assert_eq!(selected.node_id, high);
        assert!(selected.breakdown.total > 1.0);
    }

    #[test]
    fn chooser_ignores_non_open_action_nodes() {
        let mut graph = PlanGraph::from_goal("ship".to_string());
        let blocked = graph.add_node(action_node("blocked", scores(10.0)));
        let open = graph.add_node(action_node("open", scores(1.0)));
        graph.node_mut(blocked).unwrap().status = PlanStatus::Blocked;

        let selected = Scheduler::default().choose_next_action(&graph).unwrap();

        assert_eq!(selected.node_id, open);
    }

    #[test]
    fn chooser_waits_for_unsatisfied_dependencies() {
        let mut graph = PlanGraph::from_goal("ship".to_string());
        let blocker = graph.add_node(PlanNode {
            id: NodeId(0),
            kind: PlanNodeKind::Observation,
            status: PlanStatus::Open,
            label: "inspect".to_string(),
            payload: json!({}),
            action_ref: None,
            scores: ActionScores::default(),
        });
        let dependent = graph.add_node(action_node("dependent", scores(10.0)));
        let fallback = graph.add_node(action_node("fallback", scores(0.1)));
        graph.add_edge(blocker, dependent, PlanEdgeKind::DependsOn, json!({}));

        let selected = Scheduler::default().choose_next_action(&graph).unwrap();

        assert_eq!(selected.node_id, fallback);
    }

    #[test]
    fn repeated_failures_reduce_success_probability_and_total_score() {
        let clean = ActionEstimate::from_scores(&scores(1.0));
        let mut failed_scores = scores(1.0);
        failed_scores.repeated_failure_penalty = 2.0;
        let failed = ActionEstimate::from_scores(&failed_scores);

        let weights = SchedulerWeights::default();
        let clean_score = score_action(&clean, &weights);
        let failed_score = score_action(&failed, &weights);

        assert!(failed.success_probability < clean.success_probability);
        assert!(failed_score.total < clean_score.total);
    }

    #[test]
    fn uncertainty_reduces_success_probability_and_total_score() {
        let clean = ActionEstimate::from_scores(&scores(1.0));
        let mut uncertain_scores = scores(1.0);
        uncertain_scores.uncertainty_penalty = 1.5;
        let uncertain = ActionEstimate::from_scores(&uncertain_scores);

        let weights = SchedulerWeights::default();
        let clean_score = score_action(&clean, &weights);
        let uncertain_score = score_action(&uncertain, &weights);

        assert!(uncertain.success_probability < clean.success_probability);
        assert!(uncertain.success_uncertainty > clean.success_uncertainty);
        assert!(uncertain_score.total < clean_score.total);
    }

    fn assert_close(left: f64, right: f64) {
        assert!(
            (left - right).abs() < 1e-9,
            "left {left} was not close to right {right}"
        );
    }
}
