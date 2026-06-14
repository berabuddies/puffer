//! Self-report monitor-task completion.
//!
//! When a monitor creates a pending task from an *incoming* message, the user
//! often later does the thing and says so in the same chat with one of their own
//! *outgoing* messages ("拿了", "搞定了", "done"). Those outgoing messages are
//! diverted to the self-report lane (they must never enter triage, #569). This
//! module is the lane's handler: it asks a small LLM whether the user's message
//! completes an open monitor task in that conversation, and if so flips the task
//! to `completed` via the existing deterministic writeback.
//!
//! ## Token efficiency (省 token)
//!
//! The expensive LLM call is gated behind a cheap, local check: load
//! `monitor_tasks.json` and collect the open tasks belonging to *this*
//! conversation. If the conversation has no open monitor task, the handler
//! returns immediately and spends zero tokens. This is the single guarantee that
//! the normal chat traffic (the overwhelming majority) costs nothing.

use std::sync::Arc;

use puffer_config::ConfigPaths;
use puffer_core::subscription_manager;
use puffer_subscriber_runtime::EventEnvelope;
use puffer_subscriptions::{
    installed_workflow_runner, ActionSpec, RemoteSelfReportHandler, SelfReportHandler,
};
use serde_json::{json, Map, Value};

use super::handle_monitor_task_complete;
use super::monitor_completion_phrases;
use super::monitor_task_complete::COMPLETED_VIA_SELF_REPORT;
use super::monitor_task_ignore::monitor_tasks_path;

/// Built-in multilingual phrases that frequently signal a user just finished a
/// task. These are only *hints* handed to the LLM, never a hard gate: the model
/// makes the final call so paraphrases and unlisted languages still work.
const DEFAULT_COMPLETION_PHRASES: &[&str] = &[
    // Chinese
    "拿了",
    "拿到了",
    "取了",
    "交了",
    "交好了",
    "搞定了",
    "搞定",
    "弄完了",
    "弄好了",
    "办好了",
    "办完了",
    "做完了",
    "完成了",
    "处理好了",
    "已完成",
    // English
    "done",
    "did it",
    "got it",
    "picked it up",
    "handed in",
    "handed it in",
    "finished",
    "all set",
    "taken care of",
];

/// An open monitor task that belongs to the conversation under inspection.
#[derive(Debug, Clone, PartialEq, Eq)]
struct OpenTask {
    task_id: String,
    subject: String,
    description: String,
    /// Owning monitor connection slug, used to look up per-monitor completion
    /// phrases. `None` for legacy tasks without connection metadata.
    connection: Option<String>,
    /// Numeric Telegram message id of the message that originally spawned this
    /// task (written daemon-side, #630). Lets a self-report that *replies to*
    /// that exact message complete this task deterministically. `None` for
    /// legacy tasks created before source-message capture.
    source_message_id: Option<i64>,
}

/// Decides which (if any) open monitor task the user's outgoing message just
/// completed. Implementations must return the matching `task_id` or `None`.
///
/// `model` is the monitor's configured `<provider>/<model>` selector (or `None`
/// for the runtime default) so the judge reasons with the *same* model the user
/// picked for the monitor.
///
/// Defined as a trait so the cheap gate can be unit-tested with a stub judge
/// that *panics if invoked* — proving the no-open-task path never spends a call.
trait CompletionJudge {
    fn judge(
        &self,
        model: Option<&str>,
        tasks: &[OpenTask],
        message: &str,
        quote: Option<&str>,
        hints: &[String],
    ) -> Option<String>;
}

/// Builds the injected self-report handler. The judge routes through the
/// process-global workflow runner so it uses the monitor's configured model
/// (resolved per connection), never a hardcoded one. If no workflow runner is
/// installed at judge time the lane is simply a no-op.
pub(crate) fn build_self_report_handler(paths: &ConfigPaths) -> Arc<dyn SelfReportHandler> {
    let judge = RunnerCompletionJudge;
    let paths = paths.clone();
    Arc::new(RemoteSelfReportHandler::new(move |envelope| {
        process_self_report(&paths, envelope, &judge);
    }))
}

/// Core handler logic, generic over the judge so it is testable without HTTP.
fn process_self_report(paths: &ConfigPaths, envelope: &EventEnvelope, judge: &dyn CompletionJudge) {
    let Some(chat_id) = extract_chat_id(&envelope.event.payload) else {
        return;
    };
    let message = envelope.event.text.trim();
    if message.is_empty() {
        return;
    }
    // Cheap, local gate: only conversations with an open monitor task ever reach
    // the LLM. Everything else (the common case) costs nothing.
    let open_tasks = match open_tasks_for_chat(paths, &chat_id) {
        Ok(tasks) => tasks,
        Err(_) => return,
    };
    if open_tasks.is_empty() {
        return;
    }
    // A reply to a task's originating message reliably identifies *which* task
    // the user means, but NOT whether they actually finished it — a reply could
    // just as easily say "not done yet". So a reply only narrows the judge's
    // candidate set to that one task; the judge still confirms completion intent
    // before anything is completed. Any highlighted quote text is also handed to
    // the judge as disambiguation context.
    let reply = extract_reply(&envelope.event.payload);
    let quote = reply.as_ref().and_then(|reply| reply.quote_text.as_deref());
    let candidates: &[OpenTask] = reply
        .as_ref()
        .and_then(|reply| reply.message_id)
        .and_then(|reply_msg_id| {
            open_tasks
                .iter()
                .find(|task| task.source_message_id == Some(reply_msg_id))
        })
        .map(std::slice::from_ref)
        .unwrap_or(open_tasks.as_slice());

    let hints = completion_hints(paths, candidates);
    // Use whatever model the user configured for this monitor; falls back to the
    // runtime default when the monitor has no explicit model.
    let model = monitor_model_for_open_tasks(candidates);
    let Some(task_id) = judge.judge(model.as_deref(), candidates, message, quote, &hints) else {
        return;
    };
    if !candidates.iter().any(|task| task.task_id == task_id) {
        // The judge must only complete a task it was shown; ignore stray ids.
        return;
    }
    complete_task(paths, &task_id);
}

/// Flips a monitor task to `completed` via the deterministic daemon writeback,
/// recording that it was completed by self-report (which bypasses the
/// human-approval gate the reply-writeback path enforces).
fn complete_task(paths: &ConfigPaths, task_id: &str) {
    let _ = handle_monitor_task_complete(
        paths,
        &json!({
            "task_id": task_id,
            "completed_via": COMPLETED_VIA_SELF_REPORT,
        }),
    );
}

/// Completion-phrase hints offered to the judge: the built-in multilingual
/// defaults plus any per-monitor configured phrases for the connections the open
/// tasks belong to. Deduplicated; the defaults always come first.
fn completion_hints(paths: &ConfigPaths, tasks: &[OpenTask]) -> Vec<String> {
    let mut hints: Vec<String> = DEFAULT_COMPLETION_PHRASES
        .iter()
        .map(|phrase| (*phrase).to_string())
        .collect();
    let mut seen: std::collections::BTreeSet<String> = hints.iter().cloned().collect();
    let mut connections: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
    for task in tasks {
        if let Some(connection) = task.connection.as_deref() {
            connections.insert(connection);
        }
    }
    for connection in connections {
        for phrase in monitor_completion_phrases::load_completion_phrases(paths, connection) {
            if seen.insert(phrase.clone()) {
                hints.push(phrase);
            }
        }
    }
    hints
}

/// The `<provider>/<model>` selector the user configured for the monitor that
/// owns these open tasks, or `None` when the monitor has no explicit model (the
/// runner then applies the same default it uses for triage). All open tasks for
/// one conversation share a monitor connection, so the first one decides.
fn monitor_model_for_open_tasks(tasks: &[OpenTask]) -> Option<String> {
    let connection = tasks.iter().find_map(|task| task.connection.as_deref())?;
    let manager = subscription_manager().ok()?;
    manager
        .store()
        .list()
        .into_iter()
        .find(|binding| binding.connection_slug == connection)
        .and_then(|binding| match binding.action {
            ActionSpec::TriageAgent { model, .. } => model,
            _ => None,
        })
}

/// Loads the open monitor tasks whose conversation matches `chat_id`. Open means
/// not in a terminal state (`completed`/`cancelled`) and not ignored.
fn open_tasks_for_chat(paths: &ConfigPaths, chat_id: &str) -> anyhow::Result<Vec<OpenTask>> {
    let path = monitor_tasks_path(paths);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let store: Value = serde_json::from_str(&raw)?;
    let Some(tasks) = store.get("tasks").and_then(Value::as_array) else {
        return Ok(Vec::new());
    };
    Ok(tasks
        .iter()
        .filter(|task| task_is_open(task))
        .filter(|task| task_chat_id(task).as_deref() == Some(chat_id))
        .filter_map(open_task_from_value)
        .collect())
}

fn task_is_open(task: &Value) -> bool {
    let terminal = matches!(
        task.get("status").and_then(Value::as_str),
        Some("completed") | Some("cancelled")
    );
    if terminal {
        return false;
    }
    !task_metadata_bool(task, "ignored")
}

fn open_task_from_value(task: &Value) -> Option<OpenTask> {
    let task_id = task_string(task, &["task_id", "taskId", "id"])?;
    Some(OpenTask {
        task_id,
        subject: task_string(task, &["subject"]).unwrap_or_default(),
        description: task_string(task, &["description"]).unwrap_or_default(),
        connection: task_connection(task),
        source_message_id: task_source_message_id(task),
    })
}

/// The numeric Telegram message id of the message that originally spawned this
/// task. The daemon writes it to `metadata.source_message_id` (and mirrors it on
/// `source_context.message_id`) for reply threading (#630); we reuse it to match
/// a self-report that replies to that exact message.
fn task_source_message_id(task: &Value) -> Option<i64> {
    let metadata = task.get("metadata").and_then(Value::as_object)?;
    if let Some(id) = metadata.get("source_message_id").and_then(Value::as_i64) {
        return Some(id);
    }
    metadata
        .get("source_context")
        .and_then(Value::as_object)
        .and_then(|context| context.get("message_id"))
        .and_then(Value::as_i64)
}

/// Pulls the owning monitor connection slug from a task's metadata, tolerating
/// the nested `monitor`/`payload` containers monitor tasks may use. Used to look
/// up per-monitor completion phrases.
fn task_connection(task: &Value) -> Option<String> {
    let metadata = task.get("metadata").and_then(Value::as_object)?;
    const KEYS: &[&str] = &["monitor_connection", "monitorConnection", "connection"];
    for key in KEYS {
        if let Some(value) = metadata.get(*key).and_then(value_to_id_string) {
            return Some(value);
        }
    }
    for container in ["monitor", "payload", "event_payload", "eventPayload"] {
        if let Some(inner) = metadata.get(container).and_then(Value::as_object) {
            for key in KEYS {
                if let Some(value) = inner.get(*key).and_then(value_to_id_string) {
                    return Some(value);
                }
            }
        }
    }
    None
}

/// Pulls the conversation id from a task's metadata, tolerating string or number
/// shapes and the nested `monitor`/`payload` containers monitor tasks may use.
fn task_chat_id(task: &Value) -> Option<String> {
    let metadata = task.get("metadata").and_then(Value::as_object)?;
    metadata_chat_id(metadata)
}

fn metadata_chat_id(metadata: &Map<String, Value>) -> Option<String> {
    const KEYS: &[&str] = &["chat_id", "chatId"];
    for key in KEYS {
        if let Some(value) = metadata.get(*key) {
            if let Some(id) = value_to_id_string(value) {
                return Some(id);
            }
        }
    }
    for container in ["monitor", "payload", "event_payload", "eventPayload"] {
        if let Some(inner) = metadata.get(container).and_then(Value::as_object) {
            for key in KEYS {
                if let Some(id) = inner.get(*key).and_then(value_to_id_string) {
                    return Some(id);
                }
            }
        }
    }
    None
}

fn value_to_id_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn task_metadata_bool(task: &Value, key: &str) -> bool {
    task.get("metadata")
        .and_then(Value::as_object)
        .and_then(|metadata| metadata.get(key))
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn task_string(task: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| task.get(*key).and_then(Value::as_str))
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn extract_chat_id(payload: &Value) -> Option<String> {
    payload.get("chat_id").and_then(value_to_id_string)
}

/// What the user's self-message replied to, if anything. Telegram replies carry
/// the replied-to `message_id` always, and a `quote_text` only when the user
/// highlighted part of the message to quote.
struct ReplyInfo {
    /// The replied-to message's numeric id, for deterministic task matching.
    message_id: Option<i64>,
    /// The highlighted quote snippet, handed to the judge as disambiguation
    /// context when no deterministic id match is found.
    quote_text: Option<String>,
}

/// Pulls reply/quote context out of a self-message payload (populated by the
/// telegram subscriber's `reply_to` field). Returns `None` when the message is
/// not a reply to another message.
fn extract_reply(payload: &Value) -> Option<ReplyInfo> {
    let reply_to = payload.get("reply_to")?;
    if reply_to.get("kind").and_then(Value::as_str) != Some("message") {
        return None;
    }
    let message_id = reply_to.get("message_id").and_then(Value::as_i64);
    let quote_text = reply_to
        .get("quote_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(ToOwned::to_owned);
    (message_id.is_some() || quote_text.is_some()).then_some(ReplyInfo {
        message_id,
        quote_text,
    })
}

/// The live judge: a single no-tools completion routed through the process
/// workflow runner, which resolves `model` to the right provider exactly like
/// the triage agent. This is what makes the judge use the *monitor's* model
/// (e.g. a local `qwen35/qwen3.5-0.8b`) instead of a hardcoded one. One open
/// task uses a strict yes/no prompt; multiple open tasks use a "which one"
/// disambiguation prompt that returns the task index.
struct RunnerCompletionJudge;

impl CompletionJudge for RunnerCompletionJudge {
    fn judge(
        &self,
        model: Option<&str>,
        tasks: &[OpenTask],
        message: &str,
        quote: Option<&str>,
        hints: &[String],
    ) -> Option<String> {
        let runner = installed_workflow_runner()?;
        match tasks {
            [] => None,
            [task] => {
                let reply = runner
                    .complete_text(model, &single_task_prompt(task, message, quote, hints))
                    .ok()?;
                reply
                    .trim()
                    .to_lowercase()
                    .starts_with('y')
                    .then(|| task.task_id.clone())
            }
            tasks => {
                let reply = runner
                    .complete_text(model, &multi_task_prompt(tasks, message, quote, hints))
                    .ok()?;
                parse_task_index(&reply).and_then(|index| {
                    (index >= 1 && index <= tasks.len())
                        .then(|| tasks[index - 1].task_id.clone())
                })
            }
        }
    }
}

fn single_task_prompt(task: &OpenTask, message: &str, quote: Option<&str>, hints: &[String]) -> String {
    format!(
        "A monitor created this open task from an earlier message in this chat:\n\
         Task: {subject}\n{description}\n\n\
         The user just sent this message in the same chat:\n{message}\n{quote_line}\n\
         Phrases that often mean the user finished the task (hints, not exhaustive): {hints}\n\n\
         Did the user just indicate they completed or did this task? Answer exactly 'yes' or 'no'.",
        subject = task.subject,
        description = task.description,
        quote_line = quote_context(quote),
        hints = hints.join(", "),
    )
}

fn multi_task_prompt(tasks: &[OpenTask], message: &str, quote: Option<&str>, hints: &[String]) -> String {
    let mut list = String::new();
    for (index, task) in tasks.iter().enumerate() {
        list.push_str(&format!(
            "{}. {}: {}\n",
            index + 1,
            task.subject,
            task.description
        ));
    }
    format!(
        "A monitor has these open tasks in this chat:\n{list}\n\
         The user just sent this message in the same chat:\n{message}\n{quote_line}\n\
         Phrases that often mean the user finished a task (hints, not exhaustive): {hints}\n\n\
         Which task did the user just complete? Reply with only its number, or '0' if none.",
        quote_line = quote_context(quote),
        hints = hints.join(", "),
    )
}

/// Renders the quoted-reply context line for a judge prompt, or an empty string
/// when the self-message did not quote anything. The quote strongly disambiguates
/// which task the user is reporting on when several are open in one chat.
fn quote_context(quote: Option<&str>) -> String {
    match quote {
        Some(text) => {
            format!("The user's message is a reply quoting this earlier message: \"{text}\"\n")
        }
        None => String::new(),
    }
}

fn parse_task_index(reply: &str) -> Option<usize> {
    let digits: String = reply
        .trim()
        .chars()
        .skip_while(|c| !c.is_ascii_digit())
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse::<usize>().ok()
}

#[cfg(test)]
#[path = "monitor_self_report_tests.rs"]
mod tests;
