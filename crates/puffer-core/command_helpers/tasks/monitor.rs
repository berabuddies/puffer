use crate::runtime::claude_tools::workflow::task_update;
use crate::AppState;
use anyhow::{Context, Result};
use puffer_config::ConfigPaths;
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use puffer_resources::LoadedResources;
use serde_json::{json, Map, Value};
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct MonitorTaskAction {
    pub(super) name: String,
    pub(super) prompt: String,
}

pub(super) fn task_is_ignored(metadata: &Map<String, Value>) -> bool {
    metadata
        .get("ignored")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata
            .get("monitor")
            .and_then(Value::as_object)
            .and_then(|monitor| monitor.get("ignored"))
            .and_then(Value::as_bool)
            .unwrap_or(false)
}

pub(super) fn task_is_monitor(metadata: &Map<String, Value>) -> bool {
    metadata
        .get("_monitor")
        .and_then(Value::as_bool)
        .unwrap_or(false)
        || metadata.contains_key("monitor_connection")
        || metadata.contains_key("monitorConnection")
        || metadata.get("monitor").and_then(Value::as_object).is_some()
}

pub(super) fn task_actions(metadata: &Map<String, Value>) -> Vec<MonitorTaskAction> {
    action_value(metadata)
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(|item| {
                    let name = string_field(item, &["actionName", "name", "title"])?;
                    let prompt = string_field(item, &["actionPrompt", "prompt"])?;
                    Some(MonitorTaskAction { name, prompt })
                })
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn task_ignore_reasons(metadata: &Map<String, Value>) -> Vec<String> {
    metadata
        .get("possibleIgnoreReasons")
        .or_else(|| metadata.get("possible_ignore_reasons"))
        .or_else(|| {
            metadata
                .get("monitor")
                .and_then(Value::as_object)
                .and_then(|monitor| monitor.get("possibleIgnoreReasons"))
        })
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::trim)
                .filter(|reason| !reason.is_empty())
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

pub(super) fn action_prompt(
    task_id: &str,
    subject: &str,
    description: &str,
    action: &MonitorTaskAction,
    metadata: &Map<String, Value>,
) -> String {
    format!(
        "Act on monitored task {task_id}: {subject}\n\nTask description:\n{description}\n\n{}\n\nSelected action: {}\n\n{}\n\nAction execution guardrails:\n- Use the task description and source context first; do not inspect Telegram history or connector state unless the task payload is missing information required to act.\n- Use the same language as the source message's primary language for any drafted reply or generated reply text.\n- If the source message is mixed-language and you cannot identify a primary language, use the user's preferred language or owner language from available profile/context. If no user language is available, preserve the source's dominant actionable language and do not default to English only because this prompt is English.\n- English source messages follow the same source-primary-language rule: English source messages should receive English reply drafts.\n- Preserve explicit product names, person names, company names, file names, commands, URLs, quoted text, and domain terms exactly; translate only surrounding explanatory prose.\n- Treat source context as the authoritative delivery target. Do not infer the recipient from task text, previous messages, or Telegram search results.\n- If a reply is required, call MonitorReplyDraft with taskId `{task_id}` and the final message for human review. Do not call MonitorReplySend or ConnectorAct directly for monitor-task replies.\n- For Telegram reply drafts, optimize for mobile readability: for longer or multi-part replies, prefer 2-4 short paragraphs or bullet points instead of one dense paragraph.\n- Put a blank line between paragraphs when splitting. Aim for one or two sentences per paragraph.\n- Avoid markdown tables or heavy formatting; use plain Telegram-readable text.\n- If this action requires research, use at most 3 web searches and 8 total research/tool steps. Make one focused search plan, reuse results you already opened, and do not repeat equivalent searches.\n\nWhen the action is ready, save the draft for task {task_id}; Bobo will ask the user to approve before anything is sent. If you need more context, inspect the connector or ask the user.",
        source_context_section(metadata),
        action.name,
        action.prompt
    )
}

pub(super) fn ignore_command(task_id: &str, reason: Option<&str>) -> String {
    match reason.map(str::trim).filter(|reason| !reason.is_empty()) {
        Some(reason) => format!("/tasks ignore {task_id} {reason}"),
        None => format!("/tasks ignore {task_id}"),
    }
}

pub(super) fn parse_ignore_args(args: &str) -> Option<(String, Option<String>)> {
    let rest = args.trim_start_matches("ignore").trim();
    if rest.is_empty() {
        return None;
    }
    let (task_id, reason) = rest
        .split_once(char::is_whitespace)
        .map(|(task_id, reason)| (task_id.trim(), reason.trim()))
        .unwrap_or((rest, ""));
    if task_id.is_empty() {
        return None;
    }
    let reason = reason
        .strip_prefix("--reason")
        .unwrap_or(reason)
        .trim()
        .trim_matches('"')
        .trim()
        .replace('\n', " ")
        .replace('\r', " ");
    Some((task_id.to_string(), (!reason.is_empty()).then_some(reason)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn action_prompt_instructs_reply_drafts_to_follow_source_language() {
        let prompt = action_prompt(
            "monitor-1",
            "Telegram: 确认上线风险清单",
            "对方要求今天 16:00 前给出风险清单。",
            &MonitorTaskAction {
                name: "Draft reply".to_string(),
                prompt: "Prepare a concise response.".to_string(),
            },
            &Map::new(),
        );

        assert!(prompt.contains("Use the same language as the source message's primary language"));
        assert!(prompt.contains("If the source message is mixed-language"));
        assert!(prompt.contains("English source messages"));
        assert!(prompt.contains("Preserve explicit product names"));
        assert!(prompt.contains("prefer 2-4 short paragraphs"));
        assert!(prompt.contains("Put a blank line between paragraphs"));
    }

    #[test]
    fn scoped_monitor_reply_prompt_instructs_source_language() {
        let prompt = include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../resources/prompts/monitor-reply-action.yaml"
        ));

        assert!(prompt.contains("Use the same language as the source message's primary language"));
        assert!(prompt.contains("If the source message is mixed-language"));
        assert!(prompt.contains("English source messages"));
        assert!(prompt.contains("Preserve explicit product names"));
        assert!(prompt.contains("prefer 2-4 short paragraphs"));
        assert!(prompt.contains("Put a blank line between paragraphs"));
    }
}

pub(super) fn ignore_task(
    state: &mut AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    args: &str,
) -> Result<String> {
    let Some((task_id, reason)) = parse_ignore_args(args) else {
        return Ok("Usage: /tasks ignore <task-id> [reason]".to_string());
    };
    let task = match super::load_task(state, &task_id) {
        Ok(task) => task,
        Err(_) => return Ok(format!("Unknown task `{task_id}`.")),
    };
    let memory_path = monitor_memory_path(state, &task);
    let reason = reason.unwrap_or_else(|| "User ignored this monitored task.".to_string());
    append_monitor_memory(&memory_path, &task, &reason)?;
    let cwd = state.cwd.clone();
    let raw = task_update::execute_task_update(
        state,
        &cwd,
        json!({
            "taskId": task_id,
            "status": "completed",
            "metadata": {
                "ignored": true,
                "ignore_reason": reason.clone(),
                "monitor_memory_path": memory_path.display().to_string()
            }
        }),
    )?;
    let payload: Value = serde_json::from_str(&raw).context("invalid TaskUpdate payload")?;
    let learner = spawn_ignore_learner(
        state,
        resources,
        providers,
        auth_store,
        &task,
        &memory_path,
        payload
            .get("success")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    );
    let learner_line = match learner {
        Ok(summary) => format!("background_learner={summary}"),
        Err(error) => format!("background_learner=<not launched: {error}>"),
    };
    Ok(format!(
        "Ignored task\n task_id={}\n reason={}\n memory_path={}\n{}",
        task.task_id,
        reason,
        memory_path.display(),
        learner_line
    ))
}

fn monitor_memory_path(state: &AppState, task: &super::WorkflowTaskView) -> PathBuf {
    task.metadata
        .get("monitor_memory_path")
        .or_else(|| task.metadata.get("monitorMemoryPath"))
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            ConfigPaths::discover(&state.cwd)
                .workspace_config_dir
                .join("runtime")
                .join("monitors")
                .join("memory.md")
        })
}

fn append_monitor_memory(path: &Path, task: &super::WorkflowTaskView, reason: &str) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create {}", parent.display()))?;
    }
    let entry = format!(
        "\n## Ignored Task: {}\n\nReason: {}\n\nTitle: {}\n\nDescription:\n{}\n",
        task.task_id, reason, task.subject, task.description
    );
    use std::io::Write as _;
    let mut file = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .with_context(|| format!("failed to open {}", path.display()))?;
    file.write_all(entry.as_bytes())
        .with_context(|| format!("failed to append {}", path.display()))
}

fn spawn_ignore_learner(
    state: &AppState,
    resources: &LoadedResources,
    providers: &ProviderRegistry,
    auth_store: &mut AuthStore,
    task: &super::WorkflowTaskView,
    memory_path: &Path,
    task_updated: bool,
) -> Result<String> {
    let prompt = format!(
        "A user ignored monitored connector task `{}`.\n\nTitle: {}\nDescription:\n{}\n\nThe ignore memory file is `{}`. Review the appended ignore entry and improve the file with a concise reusable filter or memory rule so future monitor triage skips similar messages. Only edit that memory file. Do not modify project source files.\n\nTask metadata update succeeded: {}.",
        task.task_id,
        task.subject,
        task.description,
        memory_path.display(),
        task_updated
    );
    let output = crate::runtime::execute_agent_tool_once(
        state,
        resources,
        providers,
        auth_store,
        &state.cwd,
        json!({
            "description": "Learn monitor ignore",
            "prompt": prompt,
            "subagent_type": "general-purpose",
            "name": "monitor-ignore-learner",
            "run_in_background": true,
            "max_turns": 3
        }),
    )?;
    let payload: Value = serde_json::from_str(&output).unwrap_or_else(|_| json!({}));
    Ok(payload
        .get("agentId")
        .or_else(|| payload.get("agent_id"))
        .and_then(Value::as_str)
        .unwrap_or("<unknown>")
        .to_string())
}

fn action_value(metadata: &Map<String, Value>) -> Option<&Value> {
    metadata.get("actions").or_else(|| {
        metadata
            .get("monitor")
            .and_then(Value::as_object)
            .and_then(|monitor| monitor.get("actions"))
    })
}

fn source_context_section(metadata: &Map<String, Value>) -> String {
    let Some(context) = metadata
        .get("source_context")
        .or_else(|| metadata.get("sourceContext"))
        .and_then(Value::as_object)
    else {
        return "Source context:\n- Not provided. If a reply is required, ask the user before choosing a recipient.".to_string();
    };
    let mut lines = vec!["Source context:".to_string()];
    if let Some(summary) = string_field_from_map(context, &["summary"]) {
        lines.push(format!("- {summary}"));
    }
    let connection_slug = string_field_from_map(context, &["connection_slug", "connectionSlug"]);
    let connector_slug = string_field_from_map(context, &["connector_slug", "connectorSlug"]);
    if connection_slug.is_some() || connector_slug.is_some() {
        lines.push(format!(
            "- Connection: {}{}",
            connection_slug.unwrap_or_else(|| "unknown".to_string()),
            connector_slug
                .map(|slug| format!(" ({slug})"))
                .unwrap_or_default()
        ));
    }
    let delivery_target = context
        .get("delivery_target")
        .or_else(|| context.get("deliveryTarget"))
        .and_then(Value::as_object);
    if let Some(delivery_target) = delivery_target {
        let kind = string_field_from_map(delivery_target, &["type"])
            .unwrap_or_else(|| "telegram_chat".to_string());
        let chat_id = string_field_from_map(delivery_target, &["chat_id", "chatId"]);
        lines.push(format!(
            "- Delivery target: {kind}{}",
            chat_id
                .map(|value| format!(" chat_id={value}"))
                .unwrap_or_default()
        ));
    }
    lines.push(
        "- Use this source context as the only authorized reply target for this monitor task."
            .to_string(),
    );
    lines.join("\n")
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    value
        .as_object()
        .and_then(|object| string_field_from_map(object, keys))
}

fn string_field_from_map(value: &Map<String, Value>, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| match value.get(*key) {
        Some(Value::String(value)) => {
            let trimmed = value.trim();
            (!trimmed.is_empty()).then(|| trimmed.to_string())
        }
        Some(Value::Number(value)) => Some(value.to_string()),
        _ => None,
    })
}
