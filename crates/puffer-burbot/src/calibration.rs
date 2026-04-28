//! Empirical action calibration primitives.
//!
//! This module intentionally works only with context-free execution outcomes:
//! attempt counts, success counts, latency, and cost. Contract semantics and
//! tool names stay outside the estimator so the same calibration path applies
//! to every action contract.

use serde::{Deserialize, Serialize};

const UNIFORM_PRIOR_ALPHA: f64 = 1.0;
const UNIFORM_PRIOR_BETA: f64 = 1.0;
const CREDIBLE_RANGE_Z_95: f64 = 1.96;
const NORMALIZATION_QUANTILE: f64 = 0.75;

/// A Beta posterior over an action's Bernoulli success probability.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct EmpiricalBetaPosterior {
    /// Posterior alpha parameter.
    pub(crate) alpha: f64,
    /// Posterior beta parameter.
    pub(crate) beta: f64,
}

/// An approximate central credible range for a probability estimate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct CredibleRange {
    /// Inclusive lower probability bound.
    pub(crate) lower: f64,
    /// Inclusive upper probability bound.
    pub(crate) upper: f64,
}

/// Context-free observations used to build empirical normalization scales.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct CalibrationObservation {
    /// Number of observed attempts represented by this action aggregate.
    pub(crate) attempts: u64,
    /// Average latency for the aggregate in milliseconds.
    pub(crate) average_latency_ms: f64,
    /// Average cost for the aggregate.
    pub(crate) average_cost: f64,
}

/// Empirical scales used to normalize latency and cost across action aggregates.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct CalibrationContext {
    latency_scale_ms: Option<f64>,
    cost_scale: Option<f64>,
}

/// A calibrated, context-free estimate for one action aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub(crate) struct CalibratedActionEstimate {
    /// Posterior mean probability that the action succeeds.
    pub(crate) success_probability: f64,
    /// Posterior variance of the success probability.
    pub(crate) success_variance: f64,
    /// Approximate central credible range for the success probability.
    pub(crate) success_range: CredibleRange,
    /// Width of the credible range, normalized to probability units.
    pub(crate) uncertainty: f64,
    /// Share of posterior mass contributed by observations rather than prior.
    pub(crate) confidence: f64,
    /// Mean observed latency in milliseconds.
    pub(crate) average_latency_ms: f64,
    /// Mean observed cost.
    pub(crate) average_cost: f64,
    /// Empirically normalized latency burden in the range 0..=1.
    pub(crate) normalized_latency: f64,
    /// Empirically normalized cost burden in the range 0..=1.
    pub(crate) normalized_cost: f64,
}

impl EmpiricalBetaPosterior {
    /// Builds a posterior from a uniform Beta(1, 1) prior and observed counts.
    pub(crate) fn uniform_from_counts(attempts: u64, successes: u64) -> Self {
        Self::from_counts(UNIFORM_PRIOR_ALPHA, UNIFORM_PRIOR_BETA, attempts, successes)
    }

    /// Builds a posterior from explicit prior parameters and observed counts.
    pub(crate) fn from_counts(
        prior_alpha: f64,
        prior_beta: f64,
        attempts: u64,
        successes: u64,
    ) -> Self {
        let prior_alpha = positive_or_default(prior_alpha, UNIFORM_PRIOR_ALPHA);
        let prior_beta = positive_or_default(prior_beta, UNIFORM_PRIOR_BETA);
        let successes = successes.min(attempts);
        let failures = attempts.saturating_sub(successes);
        Self {
            alpha: prior_alpha + successes as f64,
            beta: prior_beta + failures as f64,
        }
    }

    /// Returns the posterior mean success probability.
    pub(crate) fn mean(&self) -> f64 {
        self.alpha / self.total_mass()
    }

    /// Returns the posterior variance of the success probability.
    pub(crate) fn variance(&self) -> f64 {
        let total = self.total_mass();
        (self.alpha * self.beta) / (total * total * (total + 1.0))
    }

    /// Returns the posterior standard deviation of the success probability.
    pub(crate) fn standard_deviation(&self) -> f64 {
        self.variance().sqrt()
    }

    /// Returns an approximate central credible range using a normal radius.
    pub(crate) fn approximate_credible_range(&self, z_score: f64) -> CredibleRange {
        let mean = self.mean();
        let radius = z_score.abs() * self.standard_deviation();
        CredibleRange {
            lower: (mean - radius).clamp(0.0, 1.0),
            upper: (mean + radius).clamp(0.0, 1.0),
        }
    }

    /// Returns the total posterior alpha plus beta mass.
    pub(crate) fn total_mass(&self) -> f64 {
        self.alpha + self.beta
    }
}

impl CredibleRange {
    /// Returns the width of the credible range.
    pub(crate) fn width(&self) -> f64 {
        (self.upper - self.lower).clamp(0.0, 1.0)
    }
}

impl CalibrationContext {
    /// Builds normalization scales from observed action aggregates.
    pub(crate) fn from_observations<I>(observations: I) -> Self
    where
        I: IntoIterator<Item = CalibrationObservation>,
    {
        let mut latencies = Vec::new();
        let mut costs = Vec::new();
        for observation in observations {
            if observation.attempts == 0 {
                continue;
            }
            push_positive(&mut latencies, observation.average_latency_ms);
            push_positive(&mut costs, observation.average_cost);
        }
        Self {
            latency_scale_ms: positive_quantile(latencies, NORMALIZATION_QUANTILE),
            cost_scale: positive_quantile(costs, NORMALIZATION_QUANTILE),
        }
    }

    /// Estimates success, uncertainty, and normalized burdens for an action aggregate.
    pub(crate) fn estimate(
        &self,
        attempts: u64,
        successes: u64,
        average_latency_ms: f64,
        average_cost: f64,
    ) -> CalibratedActionEstimate {
        let posterior = EmpiricalBetaPosterior::uniform_from_counts(attempts, successes);
        let success_range = posterior.approximate_credible_range(CREDIBLE_RANGE_Z_95);
        CalibratedActionEstimate {
            success_probability: posterior.mean(),
            success_variance: posterior.variance(),
            success_range,
            uncertainty: success_range.width(),
            confidence: confidence_from_counts(attempts, posterior.total_mass()),
            average_latency_ms,
            average_cost,
            normalized_latency: normalize_positive(average_latency_ms, self.latency_scale_ms),
            normalized_cost: normalize_positive(average_cost, self.cost_scale),
        }
    }
}

impl CalibratedActionEstimate {
    /// Returns how far the estimate falls below an even success probability.
    pub(crate) fn failure_gap(&self) -> f64 {
        (0.5 - self.success_probability).max(0.0)
    }

    /// Returns how far the estimate rises above an even success probability.
    pub(crate) fn success_lift(&self) -> f64 {
        self.success_probability - 0.5
    }
}

fn positive_or_default(value: f64, fallback: f64) -> f64 {
    if value.is_finite() && value > 0.0 {
        value
    } else {
        fallback
    }
}

fn push_positive(values: &mut Vec<f64>, value: f64) {
    if value.is_finite() && value > 0.0 {
        values.push(value);
    }
}

fn positive_quantile(mut values: Vec<f64>, quantile: f64) -> Option<f64> {
    values.retain(|value| value.is_finite() && *value > 0.0);
    if values.is_empty() {
        return None;
    }
    values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let bounded_quantile = quantile.clamp(0.0, 1.0);
    let index = ((values.len() - 1) as f64 * bounded_quantile).round() as usize;
    values.get(index).copied()
}

fn normalize_positive(value: f64, scale: Option<f64>) -> f64 {
    let Some(scale) = scale else {
        return 0.0;
    };
    if !value.is_finite() || value <= 0.0 || !scale.is_finite() || scale <= 0.0 {
        return 0.0;
    }
    (value / (value + scale)).clamp(0.0, 1.0)
}

fn confidence_from_counts(attempts: u64, posterior_mass: f64) -> f64 {
    if attempts == 0 || !posterior_mass.is_finite() || posterior_mass <= 0.0 {
        return 0.0;
    }
    (attempts as f64 / posterior_mass).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beta_posterior_uses_uniform_prior() {
        let posterior = EmpiricalBetaPosterior::uniform_from_counts(4, 1);

        assert_close(posterior.alpha, 2.0);
        assert_close(posterior.beta, 4.0);
        assert_close(posterior.mean(), 2.0 / 6.0);
        assert_close(posterior.variance(), 8.0 / (36.0 * 7.0));
    }

    #[test]
    fn credible_range_exposes_probability_uncertainty() {
        let posterior = EmpiricalBetaPosterior::uniform_from_counts(0, 0);
        let range = posterior.approximate_credible_range(1.96);

        assert_close(range.lower, 0.0);
        assert_close(range.upper, 1.0);
        assert_close(range.width(), 1.0);
    }

    #[test]
    fn context_normalizes_latency_and_cost_against_observed_scale() {
        let context = CalibrationContext::from_observations([
            CalibrationObservation {
                attempts: 2,
                average_latency_ms: 100.0,
                average_cost: 0.01,
            },
            CalibrationObservation {
                attempts: 2,
                average_latency_ms: 900.0,
                average_cost: 0.09,
            },
        ]);

        let fast = context.estimate(2, 2, 100.0, 0.01);
        let slow = context.estimate(2, 2, 900.0, 0.09);

        assert!(slow.normalized_latency > fast.normalized_latency);
        assert!(slow.normalized_cost > fast.normalized_cost);
        assert_close(slow.normalized_latency, 0.5);
        assert_close(slow.normalized_cost, 0.5);
    }

    #[test]
    fn calibrated_estimate_combines_probability_confidence_and_uncertainty() {
        let estimate = CalibrationContext::default().estimate(4, 1, 200.0, 0.25);

        assert_close(estimate.success_probability, 2.0 / 6.0);
        assert_close(estimate.confidence, 4.0 / 6.0);
        assert!(estimate.uncertainty > 0.0);
        assert!(estimate.failure_gap() > 0.0);
        assert!(estimate.success_lift() < 0.0);
        assert_close(estimate.average_latency_ms, 200.0);
        assert_close(estimate.average_cost, 0.25);
    }

    fn assert_close(left: f64, right: f64) {
        assert!(
            (left - right).abs() < 1e-9,
            "left {left} was not close to right {right}"
        );
    }
}
