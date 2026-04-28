use crate::calibration::{CalibratedActionEstimate, CalibrationContext, CalibrationObservation};
use crate::graph::{ActionRef, ActionScores};
use crate::trace::{JsonlTraceStore, TraceEvent, TraceEventType};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const PROGRESS_CALIBRATION_SCALE: f64 = 1.0;
const UNCERTAINTY_SCORE_SCALE: f64 = 0.5;
const FAILURE_SCORE_SCALE: f64 = 1.0;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ActionTraceStats {
    actions: BTreeMap<ActionRef, ActionStats>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct ActionStats {
    pub(crate) attempts: u64,
    pub(crate) successes: u64,
    pub(crate) total_latency_ms: u64,
    pub(crate) total_cost: f64,
}

impl ActionTraceStats {
    /// Builds aggregate action stats from trace events.
    pub(crate) fn from_events(events: &[TraceEvent]) -> Self {
        let mut stats = Self::default();
        for event in events {
            if event.event_type != TraceEventType::ActionExecuted {
                continue;
            }
            let (Some(contract_id), Some(action_name)) = (&event.contract_id, &event.action_name)
            else {
                continue;
            };
            let entry = stats.actions.entry(ActionRef {
                contract_id: contract_id.clone(),
                action_name: action_name.clone(),
            });
            entry.or_default().observe(event);
        }
        stats
    }

    /// Builds aggregate action stats from every run in a trace store.
    pub(crate) fn from_trace_store(store: &JsonlTraceStore) -> Self {
        let mut stats = Self::default();
        let Ok(runs) = store.list_runs() else {
            return stats;
        };
        for run_id in runs {
            if let Ok(events) = store.events_for_run(run_id) {
                stats.merge(Self::from_events(&events));
            }
        }
        stats
    }

    /// Merges another aggregate into this aggregate.
    pub(crate) fn merge(&mut self, other: Self) {
        for (action_ref, stats) in other.actions {
            self.actions.entry(action_ref).or_default().merge(stats);
        }
    }

    /// Returns aggregate stats for one action reference.
    pub(crate) fn action_stats(&self, action_ref: &ActionRef) -> Option<&ActionStats> {
        self.actions.get(action_ref)
    }

    /// Builds empirical calibration scales from the accumulated action stats.
    pub(crate) fn calibration_context(&self) -> CalibrationContext {
        CalibrationContext::from_observations(
            self.actions
                .values()
                .map(ActionStats::calibration_observation),
        )
    }

    /// Returns the calibrated estimate for one action reference, if observed.
    pub(crate) fn calibrated_estimate(
        &self,
        action_ref: &ActionRef,
    ) -> Option<CalibratedActionEstimate> {
        let context = self.calibration_context();
        self.action_stats(action_ref)
            .map(|stats| stats.calibrated_estimate_with_context(&context))
    }

    /// Applies calibrated probability, uncertainty, latency, and cost to scores.
    pub(crate) fn adjust_scores(&self, action_ref: &ActionRef, scores: &mut ActionScores) {
        let Some(estimate) = self.calibrated_estimate(action_ref) else {
            return;
        };
        apply_calibrated_estimate(scores, &estimate);
    }
}

impl ActionStats {
    /// Returns the raw observed success rate, or an even prior without attempts.
    pub(crate) fn success_rate(&self) -> f64 {
        if self.attempts == 0 {
            return 0.5;
        }
        self.successes as f64 / self.attempts as f64
    }

    /// Returns mean observed latency in milliseconds.
    pub(crate) fn average_latency_ms(&self) -> f64 {
        if self.attempts == 0 {
            return 0.0;
        }
        self.total_latency_ms as f64 / self.attempts as f64
    }

    /// Returns mean observed cost.
    pub(crate) fn average_cost(&self) -> f64 {
        if self.attempts == 0 {
            return 0.0;
        }
        self.total_cost / self.attempts as f64
    }

    /// Returns a calibrated estimate without cross-action normalization scales.
    pub(crate) fn calibrated_estimate(&self) -> CalibratedActionEstimate {
        self.calibrated_estimate_with_context(&CalibrationContext::default())
    }

    /// Returns a calibrated estimate using the provided empirical context.
    pub(crate) fn calibrated_estimate_with_context(
        &self,
        context: &CalibrationContext,
    ) -> CalibratedActionEstimate {
        context.estimate(
            self.attempts,
            self.successes,
            self.average_latency_ms(),
            self.average_cost(),
        )
    }

    /// Returns the context-free aggregate observation for this action.
    pub(crate) fn calibration_observation(&self) -> CalibrationObservation {
        CalibrationObservation {
            attempts: self.attempts,
            average_latency_ms: self.average_latency_ms(),
            average_cost: self.average_cost(),
        }
    }

    fn observe(&mut self, event: &TraceEvent) {
        self.attempts += 1;
        if event.success == Some(true) {
            self.successes += 1;
        }
        self.total_latency_ms += event.latency_ms.unwrap_or(0);
        self.total_cost += event.cost.unwrap_or(0.0);
    }

    fn merge(&mut self, other: Self) {
        self.attempts += other.attempts;
        self.successes += other.successes;
        self.total_latency_ms += other.total_latency_ms;
        self.total_cost += other.total_cost;
    }
}

fn apply_calibrated_estimate(scores: &mut ActionScores, estimate: &CalibratedActionEstimate) {
    let progress_multiplier =
        1.0 + (estimate.success_lift() * estimate.confidence * PROGRESS_CALIBRATION_SCALE);
    scores.expected_progress *= progress_multiplier.clamp(0.5, 1.5);
    scores.uncertainty_penalty += estimate.uncertainty * UNCERTAINTY_SCORE_SCALE;
    scores.repeated_failure_penalty +=
        estimate.failure_gap() * estimate.confidence * FAILURE_SCORE_SCALE;
    scores.latency_penalty += estimate.normalized_latency * estimate.confidence;
    scores.cost_penalty += estimate.normalized_cost * estimate.confidence;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ids::RunId;
    use crate::trace::trace_event;
    use serde_json::json;

    fn executed(action_name: &str, success: bool, latency_ms: u64) -> TraceEvent {
        executed_with_cost(action_name, success, latency_ms, 0.0)
    }

    fn executed_with_cost(
        action_name: &str,
        success: bool,
        latency_ms: u64,
        cost: f64,
    ) -> TraceEvent {
        let mut event = trace_event(
            RunId::new(),
            TraceEventType::ActionExecuted,
            None,
            json!({}),
            json!({}),
        );
        event.contract_id = Some("puffer.tools".to_string());
        event.action_name = Some(action_name.to_string());
        event.success = Some(success);
        event.latency_ms = Some(latency_ms);
        event.cost = Some(cost);
        event
    }

    #[test]
    fn stats_count_action_successes_and_latency() {
        let stats = ActionTraceStats::from_events(&[
            executed("Bash", true, 100),
            executed("Bash", false, 300),
            executed("Read", true, 50),
        ]);
        let bash = stats
            .action_stats(&ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Bash".to_string(),
            })
            .unwrap();

        assert_eq!(bash.attempts, 2);
        assert_eq!(bash.successes, 1);
        assert_eq!(bash.average_latency_ms(), 200.0);
    }

    #[test]
    fn score_adjustment_penalizes_low_success_history_with_uncertainty() {
        let stats = ActionTraceStats::from_events(&[
            executed("Bash", false, 100),
            executed("Bash", false, 100),
            executed("Bash", true, 100),
        ]);
        let mut scores = ActionScores {
            expected_progress: 1.0,
            uncertainty_penalty: 1.0,
            ..ActionScores::default()
        };

        stats.adjust_scores(
            &ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Bash".to_string(),
            },
            &mut scores,
        );

        assert!(scores.expected_progress < 1.0);
        assert!(scores.repeated_failure_penalty > 0.0);
        assert!(scores.uncertainty_penalty > 1.0);
    }

    #[test]
    fn calibrated_estimate_uses_beta_posterior_and_confidence() {
        let stats = ActionStats {
            attempts: 4,
            successes: 1,
            total_latency_ms: 400,
            total_cost: 2.0,
        };

        let estimate = stats.calibrated_estimate();

        assert_eq!(estimate.success_probability, 2.0 / 6.0);
        assert_eq!(estimate.confidence, 4.0 / 6.0);
        assert!(estimate.success_variance > 0.0);
        assert!(estimate.success_range.lower < estimate.success_probability);
        assert!(estimate.success_range.upper > estimate.success_probability);
        assert!(estimate.uncertainty > 0.0);
        assert_eq!(estimate.average_latency_ms, 100.0);
        assert_eq!(estimate.average_cost, 0.5);
    }

    #[test]
    fn score_adjustment_normalizes_cost_and_latency_empirically() {
        let stats = ActionTraceStats::from_events(&[
            executed_with_cost("Fast", true, 100, 0.01),
            executed_with_cost("Fast", true, 100, 0.01),
            executed_with_cost("Slow", true, 900, 0.09),
            executed_with_cost("Slow", true, 900, 0.09),
        ]);
        let mut fast_scores = ActionScores::default();
        let mut slow_scores = ActionScores::default();

        stats.adjust_scores(
            &ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Fast".to_string(),
            },
            &mut fast_scores,
        );
        stats.adjust_scores(
            &ActionRef {
                contract_id: "puffer.tools".to_string(),
                action_name: "Slow".to_string(),
            },
            &mut slow_scores,
        );

        assert!(slow_scores.latency_penalty > fast_scores.latency_penalty);
        assert!(slow_scores.cost_penalty > fast_scores.cost_penalty);
    }
}
