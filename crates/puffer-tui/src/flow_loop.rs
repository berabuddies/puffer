//! Loop / Maximize / Minimize command handling.

use crate::state::{LoopKind, LoopState, LoopStatus};
use crate::TuiState;
use anyhow::Result;
use puffer_core::{AppState, MessageRole};
use puffer_session_store::SessionStore;
use regex_lite::Regex;
use std::time::{Duration, Instant};

use super::emit_system_message;

pub(crate) const DEFAULT_MAX_ITERATIONS: usize = 50;
const CONVERGENCE_WINDOW: usize = 3;

/// Intercepts `/loop`, `/maximize`, `/minimize` (and `/loop stop`) before
/// they reach `dispatch_command`.  Returns `true` when the input was consumed.
pub(super) fn try_handle_loop_command(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
    submitted: &str,
) -> Result<bool> {
    let without_slash = match submitted.strip_prefix('/') {
        Some(rest) => rest,
        None => return Ok(false),
    };
    let (name, args) = without_slash
        .split_once(' ')
        .map(|(n, a)| (n, a.trim()))
        .unwrap_or((without_slash, ""));

    match name {
        "loop" => {
            if args == "stop" || args == "cancel" {
                return stop_active_loop(state, session_store, tui);
            }
            if args.is_empty() {
                emit_system_message(
                    state,
                    session_store,
                    "Usage: /loop <interval> <prompt>  (e.g. /loop 5m check deploy)".to_string(),
                )?;
                return Ok(true);
            }
            let (interval, prompt) = parse_loop_args(args);
            if prompt.is_empty() {
                emit_system_message(
                    state,
                    session_store,
                    "Usage: /loop <interval> <prompt>".to_string(),
                )?;
                return Ok(true);
            }
            let loop_state = LoopState {
                kind: LoopKind::Loop,
                prompt: prompt.to_string(),
                iteration: 1,
                max_iterations: DEFAULT_MAX_ITERATIONS,
                interval: Some(interval),
                next_fire: None,
                target_history: Vec::new(),
                status: LoopStatus::Running,
            };
            tui.active_loop = Some(loop_state);
            emit_system_message(
                state,
                session_store,
                format!(
                    "Loop started: every {}s → \"{prompt}\" (max {DEFAULT_MAX_ITERATIONS} iterations)",
                    interval.as_secs()
                ),
            )?;
            tui.enqueue_prompt(prompt.to_string());
            Ok(true)
        }
        "maximize" | "max" => start_optimize(state, session_store, tui, args, true),
        "minimize" | "min" => start_optimize(state, session_store, tui, args, false),
        _ => Ok(false),
    }
}

fn start_optimize(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
    args: &str,
    maximize: bool,
) -> Result<bool> {
    if args == "stop" || args == "cancel" {
        return stop_active_loop(state, session_store, tui);
    }
    let (metric, prompt) = match args.split_once(' ') {
        Some((m, p)) => (m.trim(), p.trim()),
        None => {
            let verb = if maximize { "maximize" } else { "minimize" };
            emit_system_message(
                state,
                session_store,
                format!("Usage: /{verb} <metric> <prompt>"),
            )?;
            return Ok(true);
        }
    };
    if prompt.is_empty() {
        let verb = if maximize { "maximize" } else { "minimize" };
        emit_system_message(
            state,
            session_store,
            format!("Usage: /{verb} <metric> <prompt>"),
        )?;
        return Ok(true);
    }
    let kind = if maximize {
        LoopKind::Maximize(metric.to_string())
    } else {
        LoopKind::Minimize(metric.to_string())
    };
    let verb = if maximize { "Maximize" } else { "Minimize" };
    let loop_state = LoopState {
        kind,
        prompt: prompt.to_string(),
        iteration: 1,
        max_iterations: DEFAULT_MAX_ITERATIONS,
        interval: None,
        next_fire: None,
        target_history: Vec::new(),
        status: LoopStatus::Running,
    };
    tui.active_loop = Some(loop_state);
    emit_system_message(
        state,
        session_store,
        format!("{verb} \"{metric}\" started (max {DEFAULT_MAX_ITERATIONS} iterations)"),
    )?;
    let enhanced = build_optimization_prompt(prompt, metric, maximize, 1, &[]);
    tui.enqueue_prompt(enhanced);
    Ok(true)
}

fn stop_active_loop(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
) -> Result<bool> {
    if tui.active_loop.take().is_some() {
        emit_system_message(state, session_store, "Loop stopped.".to_string())?;
    } else {
        emit_system_message(state, session_store, "No active loop to stop.".to_string())?;
    }
    Ok(true)
}

/// Called after a provider turn completes. Advances the loop to the next
/// iteration or marks it as completed.
pub(crate) fn advance_loop_after_turn(
    state: &mut AppState,
    session_store: &SessionStore,
    tui: &mut TuiState,
) -> Result<()> {
    let Some(ref mut loop_state) = tui.active_loop else {
        return Ok(());
    };
    if !matches!(loop_state.status, LoopStatus::Running) {
        return Ok(());
    }
    match &loop_state.kind {
        LoopKind::Loop => {
            if loop_state.iteration >= loop_state.max_iterations {
                loop_state.status = LoopStatus::Completed("reached max iterations".to_string());
                emit_system_message(
                    state,
                    session_store,
                    "Loop completed: max iterations reached.".to_string(),
                )?;
                return Ok(());
            }
            loop_state.iteration += 1;
            if let Some(interval) = loop_state.interval {
                loop_state.next_fire = Some(Instant::now() + interval);
                loop_state.status = LoopStatus::WaitingInterval;
            }
        }
        LoopKind::Maximize(metric) | LoopKind::Minimize(metric) => {
            let maximize = matches!(loop_state.kind, LoopKind::Maximize(_));
            let metric = metric.clone();

            if let Some(value) = extract_metric_from_transcript(&state.transcript, &metric) {
                loop_state.target_history.push(value);
            }

            if loop_state.iteration >= loop_state.max_iterations {
                loop_state.status = LoopStatus::Completed("reached max iterations".to_string());
                emit_system_message(
                    state,
                    session_store,
                    format!(
                        "Optimization completed: max iterations reached. Final values: {:?}",
                        loop_state.target_history
                    ),
                )?;
                return Ok(());
            }

            if has_converged(&loop_state.target_history) {
                loop_state.status = LoopStatus::Completed("converged".to_string());
                let last = loop_state.target_history.last().copied().unwrap_or(0.0);
                emit_system_message(
                    state,
                    session_store,
                    format!(
                        "Optimization converged at {metric} = {last:.4} after {} iterations.",
                        loop_state.iteration
                    ),
                )?;
                return Ok(());
            }

            loop_state.iteration += 1;
            let enhanced = build_optimization_prompt(
                &loop_state.prompt,
                &metric,
                maximize,
                loop_state.iteration,
                &loop_state.target_history,
            );
            tui.enqueue_prompt(enhanced);
        }
    }
    Ok(())
}

/// Fires the next `/loop` iteration when the interval timer expires.
pub(crate) fn check_loop_interval(tui: &mut TuiState) {
    let has_pending = tui.has_pending_submit();
    let should_fire = match tui.active_loop {
        Some(ref loop_state) => {
            !has_pending
                && matches!(loop_state.status, LoopStatus::WaitingInterval)
                && loop_state
                    .next_fire
                    .map(|t| Instant::now() >= t)
                    .unwrap_or(false)
        }
        None => false,
    };
    if should_fire {
        let loop_state = tui.active_loop.as_mut().unwrap();
        loop_state.next_fire = None;
        loop_state.status = LoopStatus::Running;
        let prompt = loop_state.prompt.clone();
        tui.enqueue_prompt(prompt);
    }
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

pub(super) fn parse_loop_args(args: &str) -> (Duration, &str) {
    let (first, rest) = args.split_once(' ').unwrap_or((args, ""));
    if let Some(duration) = parse_duration(first) {
        (duration, rest)
    } else {
        (Duration::from_secs(600), args)
    }
}

pub(super) fn parse_duration(token: &str) -> Option<Duration> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }
    let (digits, suffix) = if token.ends_with(|c: char| c.is_ascii_alphabetic()) {
        let split = token.len() - 1;
        (&token[..split], &token[split..])
    } else {
        (token, "s")
    };
    let value: u64 = digits.parse().ok()?;
    let secs = match suffix {
        "s" => value.max(1),
        "m" => value * 60,
        "h" => value * 3600,
        "d" => value * 86400,
        _ => return None,
    };
    Some(Duration::from_secs(secs))
}

pub(super) fn build_optimization_prompt(
    base_prompt: &str,
    metric_name: &str,
    maximize: bool,
    iteration: usize,
    history: &[f64],
) -> String {
    let direction = if maximize { "maximize" } else { "minimize" };
    let history_str = if history.is_empty() {
        "no measurements yet".to_string()
    } else {
        history
            .iter()
            .map(|v| format!("{v:.4}"))
            .collect::<Vec<_>>()
            .join(" → ")
    };
    let trend_hint = if history.len() >= 2 {
        let last = history[history.len() - 1];
        let prev = history[history.len() - 2];
        let delta = last - prev;
        if delta.abs() < f64::EPSILON {
            " (stalled — try a different approach)".to_string()
        } else {
            let arrow = if delta > 0.0 { "↑" } else { "↓" };
            format!(" (last delta: {arrow}{delta:+.4})")
        }
    } else {
        String::new()
    };
    format!(
        "{base_prompt}\n\
         \n\
         ---\n\
         Optimization context: iteration {iteration}/{DEFAULT_MAX_ITERATIONS}, goal is to {direction} the metric \"{metric_name}\".\n\
         Previous \"{metric_name}\" values: {history_str}{trend_hint}\n\
         When you are done, output the measured value on its own line in this exact format:\n\
         [[METRIC:{metric_name}=<number>]]"
    )
}

fn extract_metric_from_transcript(
    transcript: &[puffer_core::RenderedMessage],
    metric_name: &str,
) -> Option<f64> {
    for msg in transcript.iter().rev() {
        if msg.role != MessageRole::Assistant {
            continue;
        }
        if let Some(value) = extract_metric_value(&msg.text, metric_name) {
            return Some(value);
        }
        break;
    }
    None
}

pub(super) fn extract_metric_value(text: &str, metric_name: &str) -> Option<f64> {
    let pattern = format!(
        r"\[\[METRIC:{}\s*=\s*([^\]]+)\]\]",
        regex_lite::escape(metric_name)
    );
    let re = Regex::new(&pattern).ok()?;
    let caps = re.captures(text)?;
    let m = caps.get(1)?;
    let value = m.as_str().trim().parse::<f64>().ok()?;
    // Reject NaN/Infinity which would corrupt convergence detection.
    if value.is_nan() || value.is_infinite() {
        return None;
    }
    Some(value)
}

pub(super) fn has_converged(history: &[f64]) -> bool {
    if history.len() < CONVERGENCE_WINDOW {
        return false;
    }
    let tail = &history[history.len() - CONVERGENCE_WINDOW..];
    let first = tail[0];
    tail.iter().all(|v| (v - first).abs() < f64::EPSILON)
}
