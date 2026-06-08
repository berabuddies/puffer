//! Human-like timing and pointer-path helpers used when driving the WeChat
//! desktop, so input does not look like a metronomic bot.
//!
//! WeChat (like other chat clients) can flag accounts that act with machine
//! precision. These helpers add the small, bounded irregularities a real user
//! produces: variable think-time between actions, jittery typing cadence, a
//! curved multi-step mouse path instead of a teleport, and a few pixels of
//! randomness on click targets. All bounds are conservative and configurable
//! via `WECHAT_HUMAN_*` env vars; everything here is pure and unit-tested.

use rand::Rng;
use std::time::Duration;

/// Tunable bounds for human-like behaviour. Read once from the environment.
#[derive(Debug, Clone)]
pub(crate) struct Human {
    /// Inclusive think-time range between discrete actions, milliseconds.
    think_ms: (u64, u64),
    /// Inclusive extra pause before pressing send, milliseconds.
    pre_send_ms: (u64, u64),
}

impl Default for Human {
    fn default() -> Self {
        Self {
            think_ms: (450, 2200),
            pre_send_ms: (600, 2600),
        }
    }
}

impl Human {
    /// Builds bounds from `WECHAT_HUMAN_THINK_MS` / `_PRESEND_MS` (each
    /// `"min,max"`), falling back to conservative defaults. (Text is delivered by
    /// clipboard paste, not per-keystroke typing, so there is no type-delay knob.)
    pub(crate) fn from_env() -> Self {
        let mut human = Self::default();
        if let Some(range) = parse_range_env("WECHAT_HUMAN_THINK_MS") {
            human.think_ms = range;
        }
        if let Some(range) = parse_range_env("WECHAT_HUMAN_PRESEND_MS") {
            human.pre_send_ms = range;
        }
        human
    }

    /// A randomized "thinking" pause between two discrete UI actions.
    pub(crate) fn think_time(&self) -> Duration {
        Duration::from_millis(rand_in(self.think_ms))
    }

    /// A randomized pause just before pressing send, modelling re-reading.
    pub(crate) fn pre_send_pause(&self) -> Duration {
        Duration::from_millis(rand_in(self.pre_send_ms))
    }
}

/// Builds a curved, multi-step pointer path from `from` to `to` so the cursor
/// glides like a hand rather than jumping. Uses a quadratic Bezier with a
/// randomly offset control point and 8-18 steps; the final point is `to`.
pub(crate) fn mouse_path(from: (i32, i32), to: (i32, i32)) -> Vec<(i32, i32)> {
    let mut rng = rand::thread_rng();
    let steps = rng.gen_range(8..=18);
    // Control point: midpoint pushed perpendicular-ish by a random amount so
    // the arc bows to one side, scaled to the travel distance.
    let mid_x = (from.0 + to.0) as f64 / 2.0;
    let mid_y = (from.1 + to.1) as f64 / 2.0;
    let dx = (to.0 - from.0) as f64;
    let dy = (to.1 - from.1) as f64;
    let dist = (dx * dx + dy * dy).sqrt().max(1.0);
    let bow = rng.gen_range(-0.18..0.18) * dist;
    // Perpendicular unit vector (-dy, dx)/dist.
    let ctrl_x = mid_x + (-dy / dist) * bow;
    let ctrl_y = mid_y + (dx / dist) * bow;

    let mut path = Vec::with_capacity(steps as usize);
    for i in 1..=steps {
        let t = i as f64 / steps as f64;
        let inv = 1.0 - t;
        // Quadratic Bezier: (1-t)^2 P0 + 2(1-t)t C + t^2 P1.
        let x = inv * inv * from.0 as f64 + 2.0 * inv * t * ctrl_x + t * t * to.0 as f64;
        let y = inv * inv * from.1 as f64 + 2.0 * inv * t * ctrl_y + t * t * to.1 as f64;
        // A pixel of tremor on intermediate points (never on the last one).
        let (jx, jy) = if i < steps {
            (rng.gen_range(-1..=1), rng.gen_range(-1..=1))
        } else {
            (0, 0)
        };
        path.push((x.round() as i32 + jx, y.round() as i32 + jy));
    }
    // Guarantee the path ends exactly on target.
    if let Some(last) = path.last_mut() {
        *last = to;
    }
    path
}

/// Nudges a click target by a few pixels within `radius` so repeated clicks do
/// not land on the exact same pixel.
pub(crate) fn jitter_target(point: (i32, i32), radius: i32) -> (i32, i32) {
    if radius <= 0 {
        return point;
    }
    let mut rng = rand::thread_rng();
    (
        point.0 + rng.gen_range(-radius..=radius),
        point.1 + rng.gen_range(-radius..=radius),
    )
}

/// Short delay between successive mouse-move sub-steps, milliseconds.
pub(crate) fn step_delay() -> Duration {
    Duration::from_millis(rand::thread_rng().gen_range(8..=26))
}

/// Returns a random value within an inclusive `(min, max)` range; tolerant of a
/// reversed or degenerate range.
fn rand_in(range: (u64, u64)) -> u64 {
    let (lo, hi) = (range.0.min(range.1), range.0.max(range.1));
    if lo == hi {
        return lo;
    }
    rand::thread_rng().gen_range(lo..=hi)
}

/// Parses an env var of the form `"min,max"` into a `(u64, u64)` range.
fn parse_range_env(key: &str) -> Option<(u64, u64)> {
    let raw = std::env::var(key).ok()?;
    let (a, b) = raw.split_once(',')?;
    Some((a.trim().parse().ok()?, b.trim().parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mouse_path_starts_near_source_ends_on_target() {
        let path = mouse_path((0, 0), (200, 100));
        assert!(path.len() >= 8 && path.len() <= 18);
        // Last point is exactly the target.
        assert_eq!(*path.last().unwrap(), (200, 100));
        // Monotonic-ish progress: the path should not collapse to one point.
        assert!(path.iter().any(|&(x, _)| x > 10 && x < 190));
    }

    #[test]
    fn mouse_path_degenerate_same_point() {
        let path = mouse_path((50, 50), (50, 50));
        assert_eq!(*path.last().unwrap(), (50, 50));
    }

    #[test]
    fn jitter_target_within_radius() {
        for _ in 0..50 {
            let (x, y) = jitter_target((100, 100), 4);
            assert!((96..=104).contains(&x));
            assert!((96..=104).contains(&y));
        }
        assert_eq!(jitter_target((7, 7), 0), (7, 7));
    }

    #[test]
    fn ranges_are_within_bounds() {
        let human = Human::default();
        for _ in 0..50 {
            let think = human.think_time().as_millis() as u64;
            assert!((450..=2200).contains(&think));
            let presend = human.pre_send_pause().as_millis() as u64;
            assert!((600..=2600).contains(&presend));
        }
    }

    #[test]
    fn rand_in_handles_reversed_and_equal() {
        assert_eq!(rand_in((5, 5)), 5);
        let v = rand_in((10, 2));
        assert!((2..=10).contains(&v));
    }
}
