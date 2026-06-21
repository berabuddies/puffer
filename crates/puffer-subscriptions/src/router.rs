//! Workflow binding router — the loop that consumes connector events and
//! invokes matching workflow bindings.

use crate::action::ActionDispatcher;
use crate::classify::{Classifier, ClassifyDecision};
use crate::contacts::contact_filter_matches;
use crate::history::{
    now_ms, DedupDecision, WorkflowActionLog, WorkflowBindingRunStatus, WorkflowHistoryStore,
    MAX_FAILED_ATTEMPTS,
};
use crate::self_gate::{SelfMessageGate, SELF_MESSAGE_KIND};
use crate::spec::{
    filter_matches, ActionSpec, FilterSpec, WorkflowBindingSpec, WorkflowBindingStatus,
};
use crate::store::WorkflowBindingStore;
use puffer_subscriber_runtime::EventEnvelope;
use serde_json::Value;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::Arc;

mod monitor_digest;
mod runtime;

pub(crate) use monitor_digest::MonitorDigestQueue;
pub use runtime::{RouterStats, SubscriptionRouter};
const MONITOR_RUNTIME_LANGUAGE_POLICY_MARKER: &str = "Monitor source-language runtime guard";
const MONITOR_RUNTIME_LANGUAGE_POLICY: &str = r#"Monitor source-language runtime guard:
- This guard is authoritative for monitor-created task output, including older persisted monitor prompts.
- Before creating or updating a monitor task, identify the source event text's primary natural language.
- Task subject, description, actions[].actionPrompt, possibleIgnoreReasons, and user-facing reply or draft text MUST use that primary language.
- For Chinese source text, write those fields in Chinese. Do not translate them into English just because the workflow prompt, schemas, or tool names are English.
- For English source text, keep those fields in English.
- If the source text is mixed-language or unclear, prefer the human/user language evident in the source text or owner context; otherwise preserve the user's wording and avoid defaulting to English.
- Preserve explicit product names, code identifiers, URLs, and quoted text exactly when appropriate.
- Copy every number, percentage, amount, date, time, duration, and identifier into task fields exactly as written in the current source event text. Never round, convert, infer, or substitute values, and never reuse values from other messages or prior context.
- When the source message contains critical values, quote the relevant sentence verbatim inside the task description instead of paraphrasing it.
- If the trigger payload contains `conversation_context`, read it before deciding what the current source event means. `conversation_context.source=telegram_server_history_cache` means bounded recent Telegram server-history messages from the same direct chat before the current trigger; `conversation_context.source=subscriber_diagnostics` means best-effort observed subscriber diagnostics that may have gaps. Use context only to disambiguate ambiguous short messages and reply intent; do not create tasks from prior context alone, do not assume diagnostics context is complete or immediately adjacent, and do not replace the current source event's numbers, deadlines, or asks with prior-message details.
- Same chat/contact is not enough to call something a duplicate. If the current source event asks a new question, changes topic, or creates a separate request, create a new monitor task even when another task from the same sender is still pending.
- Task lifecycle: for ordinary content edits via TaskUpdate, leave status unchanged. The workflow trigger carries a direction field (incoming = a contact's message, outgoing = my own). When the current message clearly indicates an existing open monitor task in THIS conversation is done/handled/resolved, complete that task with TaskUpdate status: completed. Resolution is broader than "I did it": a decisive decision, refusal, or dismissal also completes it (e.g. outgoing "不给你擦"/"不弄了" — deciding not to act still closes the loop), and an incoming contact report that it is done completes the matching task too (e.g. "擦完了"/"已处理"). Match the conversation by stored identity like chat_id; TaskList first; if ambiguous among several, complete none UNLESS the message quotes/replies to a specific earlier message (a quote is not ambiguous — match it to that task); deferrals like "还没"/"等下"/"晚点再说" never complete. If direction is outgoing, only complete or update existing tasks — never create one. You may complete a task even if it has a reply/delivery target — completing only marks it done and never sends a reply.
- When TaskUpdate changes an existing monitor task's subject, description, or activeForm, include `metadata.monitor_envelope_id` copied from the current workflow trigger and replace or clear `metadata.actions` so stale action prompts from the prior source cannot reach the executor."#;

/// Summary of processing one event envelope against workflow bindings.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct EnvelopeProcessResult {
    /// Whether at least one enabled binding matched the envelope.
    pub matched: bool,
    /// Number of matched actions that completed successfully.
    pub acted: u64,
    /// Number of matched actions that failed.
    pub failed: u64,
}

/// Processes one event envelope against the current workflow bindings.
pub fn process_envelope(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
    gate: &Arc<dyn SelfMessageGate>,
) -> bool {
    process_envelope_result(
        envelope,
        store,
        history_store,
        dispatcher,
        classifier,
        stats,
        gate,
    )
    .matched
}

/// Processes one event envelope and returns match/action/failure counts.
pub fn process_envelope_result(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
    gate: &Arc<dyn SelfMessageGate>,
) -> EnvelopeProcessResult {
    process_envelope_result_with_monitor_digest(
        envelope,
        store,
        history_store,
        dispatcher,
        classifier,
        gate,
        None,
        stats,
    )
}

/// Processes one event envelope and delays monitor triage when configured.
pub(crate) fn process_envelope_result_with_monitor_digest(
    envelope: &EventEnvelope,
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    gate: &Arc<dyn SelfMessageGate>,
    monitor_digest: Option<&MonitorDigestQueue>,
    stats: Option<&RouterStats>,
) -> EnvelopeProcessResult {
    let mut result = EnvelopeProcessResult::default();
    if envelope.event.control {
        tracing::info!(
            subscriber = %envelope.subscriber_id,
            envelope = %envelope.envelope_id,
            topic = %envelope.event.topic,
            kind = %envelope.event.kind,
            "workflow router skipped control event"
        );
        return result;
    }
    let mut topic_matched_any = false;
    for spec in store.list() {
        let topic_matches = spec.connection_slug == envelope.event.topic
            || spec
                .connector_slug
                .as_deref()
                .is_some_and(|connector_slug| connector_slug == envelope.event.topic);
        if !topic_matches {
            continue;
        }
        topic_matched_any = true;
        if spec.status == WorkflowBindingStatus::Paused {
            log_router_skip(&spec, envelope, "binding_paused");
            continue;
        }
        if event_is_self(&envelope.event) {
            // Self/outgoing events SHORT-CIRCUIT the normal filter chain
            // (dedup/contact/classify). classify can be an LLM call, so gating
            // before it is essential (#569). The gate is cheap and injected so
            // the router holds no monitor/task knowledge.
            if !gate.should_dispatch_self_message(&envelope.event) {
                log_router_skip(&spec, envelope, "self_no_open_task");
                continue;
            }
            result.matched = true;
            dispatch_one_matched_envelope(
                &spec,
                envelope,
                history_store,
                dispatcher,
                stats,
                &mut result,
            );
            continue;
        }
        if event_dedup_key_seen(history_store, &spec, envelope) {
            log_router_skip(&spec, envelope, "dedup_seen");
            continue;
        }
        if monitor_binding_should_skip_event(&spec, &envelope.event.payload) {
            log_router_skip(&spec, envelope, "monitor_muted_skip");
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_muted_skip",
                "Muted or silent notification skipped before monitor triage.",
            );
            continue;
        }
        if ignore_filter_matches(&spec, &envelope.event.text, &envelope.event.payload) {
            log_router_skip(&spec, envelope, "monitor_ignore_filter");
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_ignore_filter",
                "Matched an installed monitor ignore filter before triage.",
            );
            continue;
        }
        if !contact_filter_matches(&spec.contact_ids, &envelope.event.payload) {
            log_router_skip(&spec, envelope, "monitor_contact_filter_skip");
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_contact_filter_skip",
                "Did not match the monitor contact filter.",
            );
            continue;
        }
        if !filter_matches(
            spec.filter.as_ref(),
            &envelope.event.text,
            &envelope.event.payload,
        ) {
            log_router_skip(&spec, envelope, "monitor_filter_skip");
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_filter_skip",
                "Did not match the monitor trigger filter.",
            );
            continue;
        }
        if spec.classify_prompt.is_some() {
            match classifier.classify(&spec, &envelope.event) {
                ClassifyDecision::Pass => {}
                ClassifyDecision::Reject | ClassifyDecision::Inconclusive => {
                    log_router_skip(&spec, envelope, "monitor_classifier_skip");
                    record_monitor_router_outcome(
                        history_store,
                        &spec,
                        envelope,
                        "monitor_classifier_skip",
                        "Classifier rejected the event before monitor triage.",
                    );
                    continue;
                }
            }
        }
        result.matched = true;
        if let Some(monitor_digest) = monitor_digest.filter(|queue| queue.handles(&spec)) {
            monitor_digest.enqueue(&spec, envelope);
            record_monitor_router_outcome(
                history_store,
                &spec,
                envelope,
                "monitor_digest_queued",
                "Queued monitor event for digest triage.",
            );
            continue;
        }
        let action = effective_action_for_dispatch(&spec);
        tracing::info!(
            workflow_binding = %spec.slug,
            envelope = %envelope.envelope_id,
            topic = %envelope.event.topic,
            kind = %envelope.event.kind,
            dedup_hash = %dedup_hash(envelope),
            text_len = envelope.event.text.len(),
            action = %action_label(action.as_ref()),
            "workflow router dispatching event"
        );
        let started_at_ms = now_ms();
        let started_history_idx = history_store.and_then(|history_store| {
            match history_store.append_action_started(
                &spec,
                envelope,
                action.as_ref(),
                started_at_ms,
            ) {
                Ok(run) => Some(run.idx),
                Err(error) => {
                    tracing::warn!(
                        workflow_binding = %spec.slug,
                        envelope = %envelope.envelope_id,
                        %error,
                        "failed to persist started workflow binding run history"
                    );
                    None
                }
            }
        });
        let action_result = dispatcher.dispatch(action.as_ref(), envelope);
        let ended_at_ms = now_ms();
        if let Some(history_store) = history_store {
            let persist_result = match started_history_idx {
                Some(idx) => match history_store.complete_action_result(
                    idx,
                    action.as_ref(),
                    &action_result,
                    started_at_ms,
                    ended_at_ms,
                ) {
                    Ok(Some(_)) => Ok(()),
                    Ok(None) => history_store
                        .append_action_result(
                            &spec,
                            envelope,
                            action.as_ref(),
                            &action_result,
                            started_at_ms,
                            ended_at_ms,
                        )
                        .map(|_| ()),
                    Err(error) => Err(error),
                },
                None => history_store
                    .append_action_result(
                        &spec,
                        envelope,
                        action.as_ref(),
                        &action_result,
                        started_at_ms,
                        ended_at_ms,
                    )
                    .map(|_| ()),
            };
            if let Err(error) = persist_result {
                tracing::warn!(
                    workflow_binding = %spec.slug,
                    envelope = %envelope.envelope_id,
                    %error,
                    "failed to persist workflow binding run history"
                );
            }
        }
        if action_result.success {
            result.acted += 1;
            if let Some(stats) = stats {
                stats.events_acted.fetch_add(1, Ordering::Relaxed);
            }
            tracing::info!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                "{}",
                action_result.summary
            );
        } else {
            result.failed += 1;
            if let Some(stats) = stats {
                stats.events_failed.fetch_add(1, Ordering::Relaxed);
            }
            tracing::warn!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                "{}",
                action_result.summary
            );
        }
    }
    if !result.matched {
        tracing::info!(
            subscriber = %envelope.subscriber_id,
            envelope = %envelope.envelope_id,
            topic = %envelope.event.topic,
            kind = %envelope.event.kind,
            dedup_hash = %dedup_hash(envelope),
            text_len = envelope.event.text.len(),
            topic_matched_any,
            "workflow router produced no action for event"
        );
    }
    result
}

/// Processes a same-connection envelope batch and batches triage-agent
/// actions per matching binding.
pub fn process_envelope_batch_result(
    envelopes: &[EventEnvelope],
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    stats: Option<&RouterStats>,
    gate: &Arc<dyn SelfMessageGate>,
) -> EnvelopeProcessResult {
    process_envelope_batch_result_with_monitor_digest(
        envelopes,
        store,
        history_store,
        dispatcher,
        classifier,
        gate,
        None,
        stats,
    )
}

/// Processes an event batch and delays monitor triage when configured.
pub(crate) fn process_envelope_batch_result_with_monitor_digest(
    envelopes: &[EventEnvelope],
    store: &WorkflowBindingStore,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    classifier: &Arc<dyn Classifier>,
    gate: &Arc<dyn SelfMessageGate>,
    monitor_digest: Option<&MonitorDigestQueue>,
    stats: Option<&RouterStats>,
) -> EnvelopeProcessResult {
    let mut result = EnvelopeProcessResult::default();
    let envelopes: Vec<&EventEnvelope> = envelopes
        .iter()
        .filter(|envelope| !envelope.event.control)
        .collect();
    if envelopes.is_empty() {
        return result;
    }
    for spec in store.list() {
        if spec.status == WorkflowBindingStatus::Paused {
            continue;
        }
        let mut triage_batch = Vec::new();
        for envelope in &envelopes {
            // Self/outgoing events SHORT-CIRCUIT the normal filter chain
            // (dedup/contact/classify) on the batch path too. classify can be an
            // LLM call, so gating before it is essential (#569). Mirror the
            // single-envelope self-branch: gate-drop skips the (spec,event);
            // gate-allow dispatches immediately and bypasses the remaining
            // filters (and is never batched into the triage agent).
            if event_is_self(&envelope.event) {
                let topic_matches = spec.connection_slug == envelope.event.topic
                    || spec
                        .connector_slug
                        .as_deref()
                        .is_some_and(|connector_slug| connector_slug == envelope.event.topic);
                if !topic_matches {
                    continue;
                }
                if !gate.should_dispatch_self_message(&envelope.event) {
                    log_router_skip(&spec, envelope, "self_no_open_task");
                    continue;
                }
                result.matched = true;
                dispatch_one_matched_envelope(
                    &spec,
                    envelope,
                    history_store,
                    dispatcher,
                    stats,
                    &mut result,
                );
                continue;
            }
            let Some(prefiltered) =
                prefilter_envelope_for_spec(&spec, envelope, history_store, classifier)
            else {
                continue;
            };
            result.matched = true;
            if matches!(spec.action, ActionSpec::TriageAgent { .. }) {
                triage_batch.push(prefiltered);
                continue;
            }
            dispatch_one_matched_envelope(
                &spec,
                prefiltered,
                history_store,
                dispatcher,
                stats,
                &mut result,
            );
        }
        if !triage_batch.is_empty() {
            if let Some(monitor_digest) = monitor_digest.filter(|queue| queue.handles(&spec)) {
                monitor_digest.enqueue_batch(&spec, &triage_batch);
                for envelope in &triage_batch {
                    record_monitor_router_outcome(
                        history_store,
                        &spec,
                        envelope,
                        "monitor_digest_queued",
                        "Queued monitor event for digest triage.",
                    );
                }
            } else {
                dispatch_matched_batch(
                    &spec,
                    &triage_batch,
                    history_store,
                    dispatcher,
                    stats,
                    &mut result,
                );
            }
        }
    }
    result
}

fn prefilter_envelope_for_spec<'a>(
    spec: &WorkflowBindingSpec,
    envelope: &'a EventEnvelope,
    history_store: Option<&WorkflowHistoryStore>,
    classifier: &Arc<dyn Classifier>,
) -> Option<&'a EventEnvelope> {
    let topic_matches = spec.connection_slug == envelope.event.topic
        || spec
            .connector_slug
            .as_deref()
            .is_some_and(|connector_slug| connector_slug == envelope.event.topic);
    if !topic_matches {
        return None;
    }
    if event_dedup_key_seen(history_store, spec, envelope) {
        log_router_skip(spec, envelope, "dedup_seen");
        return None;
    }
    if monitor_binding_should_skip_event(spec, &envelope.event.payload) {
        log_router_skip(spec, envelope, "monitor_muted_skip");
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_muted_skip",
            "Muted or silent notification skipped before monitor triage.",
        );
        return None;
    }
    if ignore_filter_matches(spec, &envelope.event.text, &envelope.event.payload) {
        log_router_skip(spec, envelope, "monitor_ignore_filter");
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_ignore_filter",
            "Matched an installed monitor ignore filter before triage.",
        );
        return None;
    }
    if !filter_matches(
        spec.filter.as_ref(),
        &envelope.event.text,
        &envelope.event.payload,
    ) {
        log_router_skip(spec, envelope, "monitor_filter_skip");
        record_monitor_router_outcome(
            history_store,
            spec,
            envelope,
            "monitor_filter_skip",
            "Did not match the monitor trigger filter.",
        );
        return None;
    }
    if spec.classify_prompt.is_some() {
        match classifier.classify(spec, &envelope.event) {
            ClassifyDecision::Pass => {}
            ClassifyDecision::Reject | ClassifyDecision::Inconclusive => {
                log_router_skip(spec, envelope, "monitor_classifier_skip");
                record_monitor_router_outcome(
                    history_store,
                    spec,
                    envelope,
                    "monitor_classifier_skip",
                    "Classifier rejected the event before monitor triage.",
                );
                return None;
            }
        }
    }
    Some(envelope)
}

fn dispatch_one_matched_envelope(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
    let started_at_ms = now_ms();
    let action = effective_action_for_dispatch(spec);
    let started_history_idx = history_store.and_then(|history_store| {
        match history_store.append_action_started(spec, envelope, action.as_ref(), started_at_ms) {
            Ok(run) => Some(run.idx),
            Err(error) => {
                tracing::warn!(
                    workflow_binding = %spec.slug,
                    envelope = %envelope.envelope_id,
                    %error,
                    "failed to persist started workflow binding run history"
                );
                None
            }
        }
    });
    let action_result = dispatcher.dispatch(action.as_ref(), envelope);
    let ended_at_ms = now_ms();
    persist_action_result(
        spec,
        envelope,
        action.as_ref(),
        &action_result,
        started_at_ms,
        ended_at_ms,
        started_history_idx,
        history_store,
    );
    account_action_result(spec, envelope, &action_result, stats, result);
}

fn dispatch_matched_batch(
    spec: &WorkflowBindingSpec,
    envelopes: &[&EventEnvelope],
    history_store: Option<&WorkflowHistoryStore>,
    dispatcher: &Arc<dyn ActionDispatcher>,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
    let started_at_ms = now_ms();
    let mut started_history = HashMap::new();
    let action = effective_action_for_dispatch(spec);
    if let Some(history_store) = history_store {
        for envelope in envelopes {
            match history_store.append_action_started(
                spec,
                envelope,
                action.as_ref(),
                started_at_ms,
            ) {
                Ok(run) => {
                    started_history.insert(envelope.envelope_id.clone(), run.idx);
                }
                Err(error) => {
                    tracing::warn!(
                        workflow_binding = %spec.slug,
                        envelope = %envelope.envelope_id,
                        %error,
                        "failed to persist started workflow binding run history"
                    );
                }
            }
        }
    }
    let batch: Vec<EventEnvelope> = envelopes
        .iter()
        .map(|envelope| (*envelope).clone())
        .collect();
    tracing::info!(
        workflow_binding = %spec.slug,
        batch_size = batch.len(),
        action = %action_label(action.as_ref()),
        "workflow router dispatching event batch"
    );
    let action_result = dispatcher.dispatch_batch(action.as_ref(), &batch);
    let ended_at_ms = now_ms();
    for envelope in envelopes {
        let started_history_idx = started_history.get(&envelope.envelope_id).copied();
        persist_action_result(
            spec,
            envelope,
            action.as_ref(),
            &action_result,
            started_at_ms,
            ended_at_ms,
            started_history_idx,
            history_store,
        );
        account_action_result(spec, envelope, &action_result, stats, result);
    }
}

fn persist_action_result(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action: &ActionSpec,
    action_result: &crate::action::ActionResult,
    started_at_ms: i128,
    ended_at_ms: i128,
    started_history_idx: Option<u64>,
    history_store: Option<&WorkflowHistoryStore>,
) {
    if let Some(history_store) = history_store {
        let persist_result = match started_history_idx {
            Some(idx) => match history_store.complete_action_result(
                idx,
                action,
                action_result,
                started_at_ms,
                ended_at_ms,
            ) {
                Ok(Some(_)) => Ok(()),
                Ok(None) => history_store
                    .append_action_result(
                        spec,
                        envelope,
                        action,
                        action_result,
                        started_at_ms,
                        ended_at_ms,
                    )
                    .map(|_| ()),
                Err(error) => Err(error),
            },
            None => history_store
                .append_action_result(
                    spec,
                    envelope,
                    action,
                    action_result,
                    started_at_ms,
                    ended_at_ms,
                )
                .map(|_| ()),
        };
        if let Err(error) = persist_result {
            tracing::warn!(
                workflow_binding = %spec.slug,
                envelope = %envelope.envelope_id,
                %error,
                "failed to persist workflow binding run history"
            );
        }
    }
}

fn account_action_result(
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action_result: &crate::action::ActionResult,
    stats: Option<&RouterStats>,
    result: &mut EnvelopeProcessResult,
) {
    if action_result.success {
        result.acted += 1;
        if let Some(stats) = stats {
            stats.events_acted.fetch_add(1, Ordering::Relaxed);
        }
        tracing::info!(
            workflow_binding = %spec.slug,
            envelope = %envelope.envelope_id,
            "{}",
            action_result.summary
        );
    } else {
        result.failed += 1;
        if let Some(stats) = stats {
            stats.events_failed.fetch_add(1, Ordering::Relaxed);
        }
        tracing::warn!(
            workflow_binding = %spec.slug,
            envelope = %envelope.envelope_id,
            "{}",
            action_result.summary
        );
    }
}

fn log_router_skip(spec: &WorkflowBindingSpec, envelope: &EventEnvelope, reason: &str) {
    tracing::info!(
        workflow_binding = %spec.slug,
        envelope = %envelope.envelope_id,
        topic = %envelope.event.topic,
        kind = %envelope.event.kind,
        dedup_hash = %dedup_hash(envelope),
        text_len = envelope.event.text.len(),
        reason = %reason,
        "workflow router skipped event"
    );
}

fn dedup_hash(envelope: &EventEnvelope) -> String {
    envelope
        .event
        .dedup_key
        .as_deref()
        .map(fnv1a64_hex)
        .unwrap_or_else(|| "none".to_string())
}

fn fnv1a64_hex(value: &str) -> String {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in value.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    format!("{hash:016x}")
}

fn action_label(action: &ActionSpec) -> &'static str {
    match action {
        ActionSpec::SqliteInsert { .. } => "sqlite_insert",
        ActionSpec::FileAppend { .. } => "file_append",
        ActionSpec::ForwardMessage { .. } => "forward_message",
        ActionSpec::RunWorkflow { .. } => "run_workflow",
        ActionSpec::ConnectorAct { .. } => "connector_act",
        ActionSpec::ToolCall { .. } => "tool_call",
        ActionSpec::TriageAgent { .. } => "triage_agent",
        ActionSpec::Graph { .. } => "graph",
        ActionSpec::Unknown => "unknown",
    }
}

fn event_dedup_key_seen(
    history_store: Option<&WorkflowHistoryStore>,
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
) -> bool {
    let Some(history_store) = history_store else {
        return false;
    };
    let Some(dedup_key) = envelope.event.dedup_key.as_deref() else {
        return false;
    };
    match history_store.dedup_decision(&spec.slug, dedup_key, MAX_FAILED_ATTEMPTS) {
        DedupDecision::Allow => false,
        DedupDecision::DuplicateOrInflight => true,
        DedupDecision::BudgetExhausted => {
            // Observability is essential here: without this, a genuinely
            // long-failing message is dropped just as silently as the original
            // bug. Surfaced via the durable telegram.log / daemon tracing.
            tracing::warn!(
                workflow_binding = %spec.slug,
                dedup_key,
                max_attempts = MAX_FAILED_ATTEMPTS,
                "workflow message suppressed: retry budget exhausted (poisoned)"
            );
            true
        }
    }
}

fn monitor_binding_should_skip_event(spec: &WorkflowBindingSpec, payload: &Value) -> bool {
    if !is_monitor_binding(spec) {
        return false;
    }
    payload_bool(payload, "notification_muted") || payload_bool(payload, "notification_silent")
}

fn record_monitor_router_outcome(
    history_store: Option<&WorkflowHistoryStore>,
    spec: &WorkflowBindingSpec,
    envelope: &EventEnvelope,
    action: &str,
    summary: &str,
) {
    if !is_monitor_binding(spec) {
        return;
    }
    let Some(history_store) = history_store else {
        return;
    };
    let timestamp = now_ms();
    let log = WorkflowActionLog {
        action: action.to_string(),
        status: WorkflowBindingRunStatus::Completed,
        summary: summary.to_string(),
        started_at_ms: timestamp,
        ended_at_ms: timestamp,
        usage: None,
    };
    if let Err(error) = history_store.append_event_outcome(
        spec,
        envelope,
        log,
        WorkflowBindingRunStatus::Completed,
        timestamp,
        timestamp,
    ) {
        tracing::warn!(
            workflow_binding = %spec.slug,
            envelope = %envelope.envelope_id,
            %error,
            "failed to persist monitor router history"
        );
    }
}

fn ignore_filter_matches(spec: &WorkflowBindingSpec, text: &str, payload: &Value) -> bool {
    spec.ignore_filters
        .iter()
        .any(|filter| filter_matches(Some(filter), text, payload))
}

fn is_monitor_binding(spec: &WorkflowBindingSpec) -> bool {
    spec.slug.starts_with("monitor-")
        || (matches!(spec.action, ActionSpec::TriageAgent { .. })
            && spec.description.to_ascii_lowercase().contains("monitor"))
}

fn effective_action_for_dispatch(spec: &WorkflowBindingSpec) -> Cow<'_, ActionSpec> {
    match &spec.action {
        ActionSpec::TriageAgent { prompt, model } if is_monitor_binding(spec) => {
            Cow::Owned(ActionSpec::TriageAgent {
                prompt: monitor_language_guarded_prompt(prompt),
                model: model.clone(),
            })
        }
        _ => Cow::Borrowed(&spec.action),
    }
}

fn monitor_language_guarded_prompt(prompt: &str) -> String {
    if prompt.contains(MONITOR_RUNTIME_LANGUAGE_POLICY_MARKER) {
        return prompt.to_string();
    }
    format!("{}\n\n{MONITOR_RUNTIME_LANGUAGE_POLICY}", prompt.trim_end())
}

fn payload_bool(payload: &Value, key: &str) -> bool {
    payload.get(key).and_then(Value::as_bool).unwrap_or(false)
}

/// Whether an event is the user's own self/outgoing message. The telegram path
/// carries `payload.is_outgoing == true`; a future connector may instead tag the
/// event kind as [`SELF_MESSAGE_KIND`].
fn event_is_self(event: &puffer_subscriber_runtime::Event) -> bool {
    event.kind == SELF_MESSAGE_KIND || payload_bool(&event.payload, "is_outgoing")
}

/// Free-standing helper used by tests and by future explicit "test this
/// workflow binding" tooling. Returns whether the filter passes.
pub fn prefilter_passes(filter: Option<&FilterSpec>, text: &str) -> bool {
    filter_matches(filter, text, &serde_json::Value::Null)
}

#[cfg(test)]
include!("router_tests.rs");
#[cfg(test)]
include!("router_monitor_rule_tests.rs");
#[cfg(test)]
#[cfg(test)]
include!("router_language_guard_tests.rs");
