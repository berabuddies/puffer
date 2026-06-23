use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_subscriptions::{WorkflowBindingRun, WorkflowHistoryStore};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use time::OffsetDateTime;

const DEFAULT_LIMIT: usize = 500;
const MAX_LIMIT: usize = 2_000;
const MESSAGE_SNIPPET_CHARS: usize = 160;

/// Exports recent Telegram monitor diagnostics to a local JSON file.
pub(crate) fn handle_telegram_diagnostics_export(
    paths: &ConfigPaths,
    params: &Value,
) -> Result<Value> {
    let limit = params
        .get("limit")
        .and_then(Value::as_u64)
        .map(|value| value as usize)
        .unwrap_or(DEFAULT_LIMIT)
        .clamp(1, MAX_LIMIT);
    let output_dir = output_dir(params).unwrap_or_else(|| default_downloads_dir(paths));
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("create diagnostics output dir {}", output_dir.display()))?;

    let report = build_telegram_diagnostics_report(paths, limit)?;
    let generated_at_ms = report
        .get("generated_at_ms")
        .and_then(Value::as_i64)
        .unwrap_or_else(now_ms);
    let path = unique_report_path(&output_dir, generated_at_ms);
    fs::write(&path, serde_json::to_vec_pretty(&report)?)
        .with_context(|| format!("write {}", path.display()))?;
    Ok(json!({
        "path": path.display().to_string(),
        "message_count": report.get("messages").and_then(Value::as_array).map(Vec::len).unwrap_or(0),
        "generated_at_ms": generated_at_ms,
    }))
}

fn build_telegram_diagnostics_report(paths: &ConfigPaths, limit: usize) -> Result<Value> {
    let mut rows: BTreeMap<String, MessageDiagnostics> = BTreeMap::new();
    let diagnostic_sources = read_subscriber_diagnostics(paths, &mut rows)?;
    let workflow_history_path = paths.user_config_dir.join("workflow_history.json");
    let workflow_history = WorkflowHistoryStore::load(&workflow_history_path)
        .with_context(|| format!("load {}", workflow_history_path.display()))?;
    let mut workflow_run_count = 0usize;
    for run in workflow_history.list() {
        if !is_telegram_monitor_run(&run) {
            continue;
        }
        workflow_run_count += 1;
        if let Some(key) = dedup_key_for_run(&run) {
            rows.entry(key)
                .or_insert_with(MessageDiagnostics::default)
                .observe_run(run);
        }
    }

    let mut messages = rows
        .into_iter()
        .map(|(dedup_key, row)| row.into_json(dedup_key))
        .collect::<Vec<_>>();
    messages.sort_by(|a, b| {
        let a_ms = sortable_message_ms(a);
        let b_ms = sortable_message_ms(b);
        b_ms.cmp(&a_ms)
    });
    messages.truncate(limit);

    let generated_at_ms = now_ms();
    Ok(json!({
        "schema": "bobo.telegram_diagnostics.v1",
        "generated_at_ms": generated_at_ms,
        "generated_at": unix_ms_to_rfc3339(generated_at_ms),
        "privacy_note": "This report may include Telegram message snippets and sender/chat identifiers. Full message text is not exported.",
        "puffer_user_config_dir": paths.user_config_dir.display().to_string(),
        "workflow_history_path": workflow_history_path.display().to_string(),
        "diagnostic_sources": diagnostic_sources,
        "workflow_run_count": workflow_run_count,
        "messages": messages,
    }))
}

fn read_subscriber_diagnostics(
    paths: &ConfigPaths,
    rows: &mut BTreeMap<String, MessageDiagnostics>,
) -> Result<Vec<Value>> {
    let root = paths.user_config_dir.join("telegram-accounts");
    let mut sources = Vec::new();
    if !root.exists() {
        return Ok(sources);
    }
    for entry in fs::read_dir(&root).with_context(|| format!("read {}", root.display()))? {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let connection_slug = entry.file_name().to_string_lossy().to_string();
        for path in diagnostic_paths(&entry.path()) {
            if !path.exists() {
                continue;
            }
            let mut count = 0usize;
            let body =
                fs::read_to_string(&path).with_context(|| format!("read {}", path.display()))?;
            for line in body.lines().map(str::trim).filter(|line| !line.is_empty()) {
                let Ok(value) = serde_json::from_str::<Value>(line) else {
                    continue;
                };
                let Some(key) = dedup_key_from_value(&value) else {
                    continue;
                };
                rows.entry(key)
                    .or_insert_with(MessageDiagnostics::default)
                    .observe_diagnostic(connection_slug.clone(), path.clone(), value);
                count += 1;
            }
            sources.push(json!({
                "connection_slug": connection_slug,
                "path": path.display().to_string(),
                "records": count,
            }));
        }
    }
    Ok(sources)
}

fn diagnostic_paths(account_dir: &Path) -> [PathBuf; 2] {
    [
        account_dir.join("message-diagnostics.ndjson"),
        account_dir.join("message-diagnostics.ndjson.1"),
    ]
}

#[derive(Default)]
struct MessageDiagnostics {
    diagnostics: Vec<SubscriberDiagnostic>,
    runs: Vec<WorkflowBindingRun>,
}

impl MessageDiagnostics {
    fn observe_diagnostic(&mut self, connection_slug: String, path: PathBuf, value: Value) {
        self.diagnostics.push(SubscriberDiagnostic {
            connection_slug,
            path,
            value,
        });
    }

    fn observe_run(&mut self, run: WorkflowBindingRun) {
        self.runs.push(run);
    }

    fn into_json(mut self, dedup_key: String) -> Value {
        self.diagnostics.sort_by_key(|diag| diag.at_ms());
        self.runs.sort_by(|a, b| b.idx.cmp(&a.idx));
        let primary_diag = self.primary_diagnostic();
        let primary_run = self.runs.first();
        let trigger = primary_run.and_then(|run| run.trigger_info.as_object());
        let payload = trigger
            .and_then(|trigger| trigger.get("payload"))
            .unwrap_or(&Value::Null);
        let message_time_ms = primary_diag
            .and_then(|diag| value_i64(diag.value.get("date_ms")))
            .or_else(|| value_i64(payload.get("date_ms")));
        let received_time_ms = primary_diag
            .and_then(|diag| value_i64(diag.value.get("source_received_at_ms")))
            .or_else(|| value_i64(payload.get("subscriber_received_at_ms")))
            .or_else(|| trigger.and_then(|trigger| value_i64(trigger.get("received_at_ms"))));
        let subscriber = subscriber_json(primary_diag, primary_run);
        let filter = filter_json(primary_diag, primary_run);
        let trust_ai = trust_ai_json(primary_run);
        let raw_diagnostics = self
            .diagnostics
            .iter()
            .map(|diag| {
                json!({
                    "connection_slug": diag.connection_slug,
                    "path": diag.path.display().to_string(),
                    "record": sanitize_message_record(&diag.value),
                })
            })
            .collect::<Vec<_>>();
        let workflow_runs = self.runs.iter().map(workflow_run_json).collect::<Vec<_>>();
        json!({
            "dedup_key": dedup_key,
            "connection_slug": primary_diag.map(|diag| diag.connection_slug.clone()).or_else(|| {
                trigger.and_then(|trigger| string_value(trigger.get("connection_slug")))
            }),
            "chat_id": primary_diag
                .and_then(|diag| diag.value.get("chat_id").cloned())
                .or_else(|| payload.get("chat_id").cloned()),
            "message_id": primary_diag
                .and_then(|diag| diag.value.get("message_id").cloned())
                .or_else(|| payload.get("message_id").cloned()),
            "message_time_ms": message_time_ms,
            "message_time": message_time_ms.map(unix_ms_to_rfc3339),
            "received_time_ms": received_time_ms,
            "received_time": received_time_ms.map(unix_ms_to_rfc3339),
            "text_prefix": primary_diag
                .and_then(|diag| string_value(diag.value.get("text_prefix")))
                .or_else(|| trigger.and_then(|trigger| string_value(trigger.get("text")).map(|text| message_snippet(&text)))),
            "subscriber": subscriber,
            "filter": filter,
            "trust_ai": trust_ai,
            "workflow_runs": workflow_runs,
            "subscriber_diagnostics": raw_diagnostics,
        })
    }

    fn primary_diagnostic(&self) -> Option<&SubscriberDiagnostic> {
        self.diagnostics
            .iter()
            .find(|diag| diag.stage() == Some("emitted"))
            .or_else(|| {
                self.diagnostics
                    .iter()
                    .find(|diag| diag.stage() == Some("suppressed"))
            })
            .or_else(|| self.diagnostics.last())
    }
}

struct SubscriberDiagnostic {
    connection_slug: String,
    path: PathBuf,
    value: Value,
}

impl SubscriberDiagnostic {
    fn stage(&self) -> Option<&str> {
        self.value.get("stage").and_then(Value::as_str)
    }

    fn at_ms(&self) -> i64 {
        value_i64(self.value.get("at_ms")).unwrap_or_default()
    }
}

fn subscriber_json(diag: Option<&SubscriberDiagnostic>, run: Option<&WorkflowBindingRun>) -> Value {
    let payload = run_payload(run);
    json!({
        "received": diag.is_some() || run.is_some(),
        "source": if diag.is_some() { "message-diagnostics" } else { "workflow-history" },
        "stage": diag.and_then(SubscriberDiagnostic::stage),
        "observed_at_ms": diag.and_then(|diag| value_i64(diag.value.get("at_ms"))),
        "observed_at": diag
            .and_then(|diag| value_i64(diag.value.get("at_ms")))
            .map(unix_ms_to_rfc3339),
        "received_at_ms": diag
            .and_then(|diag| value_i64(diag.value.get("source_received_at_ms")))
            .or_else(|| payload.and_then(|payload| value_i64(payload.get("subscriber_received_at_ms")))),
        "notification_muted": diag
            .and_then(|diag| diag.value.get("notification_muted").and_then(Value::as_bool))
            .or_else(|| payload.and_then(|payload| payload.get("notification_muted").and_then(Value::as_bool))),
        "notification_silent": diag
            .and_then(|diag| diag.value.get("notification_silent").and_then(Value::as_bool))
            .or_else(|| payload.and_then(|payload| payload.get("notification_silent").and_then(Value::as_bool))),
        "suppressed": diag
            .and_then(|diag| diag.value.get("suppressed").and_then(Value::as_bool))
            .unwrap_or(false),
    })
}

fn filter_json(diag: Option<&SubscriberDiagnostic>, run: Option<&WorkflowBindingRun>) -> Value {
    if let Some(diag) = diag {
        match diag.stage() {
            Some("suppressed") => {
                return json!({
                    "filtered": true,
                    "stage": "subscriber_suppressed",
                    "summary": "Muted or silent Telegram notification was suppressed by the subscriber before monitor triage.",
                });
            }
            Some("duplicate") => {
                return json!({
                    "filtered": true,
                    "stage": "subscriber_duplicate",
                    "summary": "Telegram message was already seen by the subscriber delivery cursor.",
                });
            }
            _ => {}
        }
    }
    let Some(run) = run else {
        return json!({
            "filtered": Value::Null,
            "stage": "no_router_history",
            "summary": "Subscriber diagnostics exist, but no monitor router history row was found.",
        });
    };
    let Some(action) = run.action_log.first() else {
        return json!({
            "filtered": Value::Null,
            "stage": "missing_action_log",
            "summary": "Monitor router history row has no action log.",
        });
    };
    let filtered = matches!(
        action.action.as_str(),
        "monitor_muted_skip"
            | "monitor_ignore_filter"
            | "monitor_contact_filter_skip"
            | "monitor_filter_skip"
            | "monitor_classifier_skip"
    );
    json!({
        "filtered": filtered,
        "stage": action.action,
        "summary": action.summary,
    })
}

fn trust_ai_json(run: Option<&WorkflowBindingRun>) -> Value {
    let Some(run) = run else {
        return json!({
            "entered": false,
            "status": "not_recorded",
            "result": "No monitor router history row was found for this message.",
        });
    };
    if let Some(action) = run
        .action_log
        .iter()
        .find(|action| action.action == "triage_agent")
    {
        return json!({
            "entered": true,
            "status": action.status,
            "result": action.summary,
            "started_at_ms": action.started_at_ms,
            "ended_at_ms": action.ended_at_ms,
            "usage": action.usage,
        });
    }
    if let Some(action) = run
        .action_log
        .iter()
        .find(|action| action.action == "monitor_digest_queued")
    {
        return json!({
            "entered": false,
            "queued": true,
            "status": action.status,
            "result": action.summary,
        });
    }
    let summary = run
        .action_log
        .first()
        .map(|action| action.summary.clone())
        .unwrap_or_else(|| "Filtered before monitor triage.".to_string());
    json!({
        "entered": false,
        "status": "not_entered",
        "result": summary,
    })
}

fn workflow_run_json(run: &WorkflowBindingRun) -> Value {
    json!({
        "idx": run.idx,
        "run_id": run.run_id,
        "workflow_slug": run.workflow_slug,
        "status": run.status,
        "started_at_ms": run.started_at_ms,
        "ended_at_ms": run.ended_at_ms,
        "trigger_info": sanitize_message_record(&run.trigger_info),
        "action_summary": run.action_summary,
        "action_log": run.action_log,
    })
}

fn sanitize_message_record(value: &Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.iter().map(sanitize_message_record).collect()),
        Value::Object(map) => {
            let mut out = serde_json::Map::new();
            for (key, value) in map {
                if key == "text" {
                    out.insert("text_redacted".to_string(), Value::Bool(true));
                } else {
                    out.insert(key.clone(), sanitize_message_record(value));
                }
            }
            Value::Object(out)
        }
        _ => value.clone(),
    }
}

fn message_snippet(text: &str) -> String {
    let mut snippet = text.chars().take(MESSAGE_SNIPPET_CHARS).collect::<String>();
    if text.chars().count() > MESSAGE_SNIPPET_CHARS {
        snippet.push_str("...");
    }
    snippet
}

fn is_telegram_monitor_run(run: &WorkflowBindingRun) -> bool {
    run.workflow_slug.starts_with("monitor-telegram")
        || run
            .trigger_info
            .get("connector_slug")
            .and_then(Value::as_str)
            == Some("telegram-login")
        || run
            .trigger_info
            .get("connection_slug")
            .and_then(Value::as_str)
            .is_some_and(|slug| slug.starts_with("telegram"))
}

fn dedup_key_for_run(run: &WorkflowBindingRun) -> Option<String> {
    run.trigger_info
        .get("dedup_key")
        .and_then(Value::as_str)
        .map(ToOwned::to_owned)
        .or_else(|| dedup_key_from_value(run_payload(Some(run))?))
}

fn dedup_key_from_value(value: &Value) -> Option<String> {
    let chat_id = value.get("chat_id")?;
    let message_id = value.get("message_id")?;
    Some(format!(
        "{}:{}",
        scalar_to_string(chat_id)?,
        scalar_to_string(message_id)?
    ))
}

fn scalar_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        _ => None,
    }
}

fn run_payload(run: Option<&WorkflowBindingRun>) -> Option<&Value> {
    run.and_then(|run| run.trigger_info.get("payload"))
}

fn value_i64(value: Option<&Value>) -> Option<i64> {
    value.and_then(|value| {
        value
            .as_i64()
            .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
            .or_else(|| value.as_str().and_then(|value| value.parse::<i64>().ok()))
    })
}

fn string_value(value: Option<&Value>) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn sortable_message_ms(value: &Value) -> i64 {
    value_i64(value.get("message_time_ms"))
        .or_else(|| value_i64(value.get("received_time_ms")))
        .unwrap_or_default()
}

fn output_dir(params: &Value) -> Option<PathBuf> {
    params
        .get("output_dir")
        .or_else(|| params.get("outputDir"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
}

fn default_downloads_dir(paths: &ConfigPaths) -> PathBuf {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .or_else(|| paths.user_config_dir.parent().map(Path::to_path_buf))
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads")
}

fn unique_report_path(output_dir: &Path, generated_at_ms: i64) -> PathBuf {
    for suffix in 0..1_000 {
        let file_name = if suffix == 0 {
            format!("bobo-telegram-diagnostics-{generated_at_ms}.json")
        } else {
            format!("bobo-telegram-diagnostics-{generated_at_ms}-{suffix}.json")
        };
        let candidate = output_dir.join(file_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    output_dir.join(format!(
        "bobo-telegram-diagnostics-{generated_at_ms}-{}.json",
        std::process::id()
    ))
}

fn now_ms() -> i64 {
    OffsetDateTime::now_utc().unix_timestamp_nanos() as i64 / 1_000_000
}

fn unix_ms_to_rfc3339(ms: i64) -> String {
    let seconds = ms.div_euclid(1_000);
    let millis = ms.rem_euclid(1_000);
    let nanos = (millis * 1_000_000) as u32;
    OffsetDateTime::from_unix_timestamp(seconds)
        .and_then(|time| time.replace_nanosecond(nanos))
        .map(|time| {
            time.format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_else(|_| ms.to_string())
        })
        .unwrap_or_else(|_| ms.to_string())
}
