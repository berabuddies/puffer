use crate::events::EventEmitter;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);
const TURN_TIMEOUT: Duration = Duration::from_secs(60 * 60);

#[derive(Debug, Clone)]
pub(crate) struct CodexTurnOptions<'a> {
    pub(crate) model: Option<&'a str>,
    pub(crate) cwd: &'a str,
    pub(crate) message: &'a str,
    pub(crate) thinking_option_id: Option<&'a str>,
    pub(crate) fast_mode: bool,
    pub(crate) permission_mode: Option<&'a str>,
    pub(crate) api_key: Option<&'a str>,
    pub(crate) playwright_cdp_endpoint: Option<&'a str>,
    pub(crate) cancel: &'a std::sync::atomic::AtomicBool,
}

#[derive(Debug, Clone)]
pub(crate) struct CapturedToolEvent {
    pub(crate) call_id: String,
    pub(crate) tool_id: String,
    pub(crate) input: String,
    pub(crate) output: String,
    pub(crate) success: bool,
}

#[derive(Debug, Clone)]
pub(crate) enum CapturedTurnEvent {
    Assistant(String),
    Tool(CapturedToolEvent),
}

#[derive(Debug, Clone)]
pub(crate) struct CodexTurnOutcome {
    pub(crate) assistant_text: String,
    pub(crate) assistant_messages: Vec<String>,
    pub(crate) tools: Vec<CapturedToolEvent>,
    pub(crate) events: Vec<CapturedTurnEvent>,
}

#[derive(Debug, Clone)]
pub(crate) struct CodexModelCatalog {
    pub(crate) models: Vec<Value>,
    pub(crate) default_model: Option<String>,
}

#[derive(Debug, Clone, Default)]
struct CodexConfiguredDefaults {
    model: Option<String>,
}

enum AppServerLine {
    Stdout(Value),
    Stderr(String),
}

struct AppServerClient {
    child: Child,
    rx: Receiver<AppServerLine>,
    next_id: u64,
    permission_mode: String,
}

impl AppServerClient {
    fn spawn(
        command: &str,
        permission_mode: Option<&str>,
        api_key: Option<&str>,
        playwright_cdp_endpoint: Option<&str>,
    ) -> Result<Self> {
        let mut command_builder = Command::new(command);
        let playwright_args = playwright_mcp_args_config(playwright_cdp_endpoint)?;
        command_builder
            .args([
                "-c".to_string(),
                "mcp_servers.playwright.command=\"npx\"".to_string(),
                "-c".to_string(),
                playwright_args,
                "app-server".to_string(),
            ])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        if let Some(api_key) = api_key.filter(|value| !value.trim().is_empty()) {
            command_builder.env("OPENAI_API_KEY", api_key);
        }
        let mut child = command_builder
            .spawn()
            .with_context(|| format!("failed to spawn {command} app-server"))?;

        let stdout = child
            .stdout
            .take()
            .context("missing codex app-server stdout")?;
        let stderr = child
            .stderr
            .take()
            .context("missing codex app-server stderr")?;
        let (tx, rx) = mpsc::channel();
        {
            let tx = tx.clone();
            thread::spawn(move || {
                for line in BufReader::new(stdout)
                    .lines()
                    .map_while(std::result::Result::ok)
                {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(value) = serde_json::from_str::<Value>(&line) {
                        let _ = tx.send(AppServerLine::Stdout(value));
                    }
                }
            });
        }
        thread::spawn(move || {
            for line in BufReader::new(stderr)
                .lines()
                .map_while(std::result::Result::ok)
            {
                let _ = tx.send(AppServerLine::Stderr(line));
            }
        });

        Ok(Self {
            child,
            rx,
            next_id: 1,
            permission_mode: permission_mode.unwrap_or("workspace-write").to_string(),
        })
    }

    fn initialize(&mut self) -> Result<()> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "corbina",
                    "title": "Corbina",
                    "version": "0.0.0",
                },
                "capabilities": {
                    "experimentalApi": true,
                }
            }),
            REQUEST_TIMEOUT,
            |_, _| {},
        )?;
        self.notify("initialized", json!({}))?;
        Ok(())
    }

    fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        self.write_json(json!({"method": method, "params": params}))
    }

    fn request<F>(
        &mut self,
        method: &str,
        params: Value,
        timeout: Duration,
        mut on_notification: F,
    ) -> Result<Value>
    where
        F: FnMut(&str, &Value),
    {
        let id = self.next_id;
        self.next_id += 1;
        self.write_json(json!({"id": id, "method": method, "params": params}))?;
        loop {
            let line = self
                .rx
                .recv_timeout(timeout)
                .with_context(|| format!("timed out waiting for codex app-server `{method}`"))?;
            match line {
                AppServerLine::Stderr(line) => on_notification(
                    "stderr",
                    &json!({
                        "line": line,
                    }),
                ),
                AppServerLine::Stdout(value) => {
                    if value.get("id").and_then(Value::as_u64) == Some(id) {
                        if let Some(error) = value.get("error") {
                            bail!("{}", error_message(error));
                        }
                        return Ok(value.get("result").cloned().unwrap_or(Value::Null));
                    }
                    if value.get("id").is_some() && value.get("method").is_some() {
                        self.answer_server_request(&value)?;
                        continue;
                    }
                    if let Some(method) = value.get("method").and_then(Value::as_str) {
                        on_notification(method, value.get("params").unwrap_or(&Value::Null));
                    }
                }
            }
        }
    }

    fn next_message(&mut self, timeout: Duration) -> Result<AppServerLine> {
        Ok(self.rx.recv_timeout(timeout)?)
    }

    fn answer_server_request(&mut self, request: &Value) -> Result<()> {
        let id = request
            .get("id")
            .filter(|value| value.is_u64() || value.is_i64() || value.is_string())
            .cloned()
            .ok_or_else(|| anyhow!("missing server request id"))?;
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");
        let params = request.get("params").unwrap_or(&Value::Null);
        let result = server_request_response(method, params, &self.permission_mode);
        self.write_json(json!({"id": id, "result": result}))
    }

    fn write_json(&mut self, value: Value) -> Result<()> {
        let stdin = self
            .child
            .stdin
            .as_mut()
            .context("missing codex app-server stdin")?;
        writeln!(stdin, "{value}")?;
        stdin.flush()?;
        Ok(())
    }
}

fn server_request_response(method: &str, params: &Value, permission_mode: &str) -> Value {
    match method {
        "item/commandExecution/requestApproval" => {
            json!({"decision": approval_decision(permission_mode)})
        }
        "item/fileChange/requestApproval" => {
            json!({"decision": approval_decision(permission_mode)})
        }
        "item/permissions/requestApproval" => {
            permissions_approval_response(params, permission_mode)
        }
        "item/tool/requestUserInput" => tool_request_user_input_response(params, permission_mode),
        "mcpServer/elicitation/request" => mcp_server_elicitation_response(params, permission_mode),
        "execCommandApproval" | "applyPatchApproval" => {
            json!({"decision": legacy_review_decision(permission_mode)})
        }
        _ => json!({}),
    }
}

fn approval_decision(permission_mode: &str) -> &'static str {
    match permission_mode {
        "read-only" => "decline",
        _ => "accept",
    }
}

fn legacy_review_decision(permission_mode: &str) -> &'static str {
    match permission_mode {
        "read-only" => "denied",
        _ => "approved",
    }
}

fn permissions_approval_response(params: &Value, permission_mode: &str) -> Value {
    if permission_mode == "read-only" {
        return json!({"permissions": {}, "scope": "turn"});
    }
    let requested = params.get("permissions").unwrap_or(&Value::Null);
    let mut permissions = serde_json::Map::new();
    if let Some(network) = requested.get("network").filter(|value| !value.is_null()) {
        permissions.insert("network".to_string(), network.clone());
    }
    if let Some(file_system) = requested.get("fileSystem").filter(|value| !value.is_null()) {
        permissions.insert("fileSystem".to_string(), file_system.clone());
    }
    json!({"permissions": Value::Object(permissions), "scope": "turn"})
}

fn tool_request_user_input_response(params: &Value, permission_mode: &str) -> Value {
    let mut answers = serde_json::Map::new();
    if permission_mode == "read-only" {
        return json!({ "answers": answers });
    }

    for question in params
        .get("questions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
    {
        let Some(question_id) = question.get("id").and_then(Value::as_str) else {
            continue;
        };
        if !is_mcp_tool_approval_question_id(question_id) {
            continue;
        }
        let answer = preferred_mcp_tool_approval_answer(question).unwrap_or("Allow");
        answers.insert(
            question_id.to_string(),
            json!({
                "answers": [answer],
            }),
        );
    }

    json!({ "answers": answers })
}

fn mcp_server_elicitation_response(params: &Value, permission_mode: &str) -> Value {
    if permission_mode == "read-only" {
        return mcp_elicitation_action("decline", Value::Null, Value::Null);
    }

    if !is_mcp_tool_approval_elicitation(params) {
        return mcp_elicitation_action("cancel", Value::Null, Value::Null);
    }

    mcp_elicitation_action(
        "accept",
        Value::Null,
        json!({
            "persist": "session",
        }),
    )
}

fn mcp_elicitation_action(action: &str, content: Value, meta: Value) -> Value {
    json!({
        "action": action,
        "content": content,
        "_meta": meta,
    })
}

fn is_mcp_tool_approval_elicitation(params: &Value) -> bool {
    params
        .get("_meta")
        .and_then(Value::as_object)
        .and_then(|meta| meta.get("codex_approval_kind"))
        .and_then(Value::as_str)
        == Some("mcp_tool_call")
}

fn is_mcp_tool_approval_question_id(question_id: &str) -> bool {
    question_id
        .strip_prefix("mcp_tool_call_approval")
        .is_some_and(|suffix| suffix.starts_with('_'))
}

fn preferred_mcp_tool_approval_answer(question: &Value) -> Option<&'static str> {
    let options = question.get("options").and_then(Value::as_array)?;
    let has_label = |label: &str| {
        options.iter().any(|option| {
            option
                .get("label")
                .and_then(Value::as_str)
                .is_some_and(|value| value == label)
        })
    };
    if has_label("Allow for this session") {
        Some("Allow for this session")
    } else if has_label("Allow") {
        Some("Allow")
    } else {
        None
    }
}

impl Drop for AppServerClient {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub(crate) fn list_model_catalog(command: &str) -> Result<CodexModelCatalog> {
    let mut client = AppServerClient::spawn(command, None, None, None)?;
    client.initialize()?;
    let (models, default_model) = fetch_models_and_default(&mut client)?;
    Ok(CodexModelCatalog {
        models: models
            .into_iter()
            .map(|model| {
                let is_default = model_is_effective_default(&model, default_model.as_deref());
                model_to_desktop_model(model, is_default)
            })
            .collect(),
        default_model,
    })
}

pub(crate) fn run_turn(
    command: &str,
    events: &EventEmitter,
    channel: &str,
    ui_turn_id: &str,
    options: CodexTurnOptions<'_>,
) -> Result<CodexTurnOutcome> {
    let result = run_turn_once(command, events, channel, ui_turn_id, &options, false);
    match result {
        Ok(outcome) => Ok(outcome),
        Err(error)
            if options_has_reasoning_effort(&options)
                && is_openai_include_validation_error(&error.to_string()) =>
        {
            events.emit(
                channel.to_string(),
                json!({
                    "type": "thinking-delta",
                    "turnId": ui_turn_id,
                    "delta": "OpenAI rejected the Codex reasoning include selector; retrying without reasoning effort.\n",
                }),
            );
            run_turn_once(command, events, channel, ui_turn_id, &options, true)
        }
        Err(error) => Err(error),
    }
}

fn run_turn_once(
    command: &str,
    events: &EventEmitter,
    channel: &str,
    ui_turn_id: &str,
    options: &CodexTurnOptions<'_>,
    omit_reasoning_effort: bool,
) -> Result<CodexTurnOutcome> {
    let mut client = AppServerClient::spawn(
        command,
        options.permission_mode,
        options.api_key,
        options.playwright_cdp_endpoint,
    )?;
    client.initialize()?;
    let (models, default_model) = fetch_models_and_default(&mut client)?;
    let model = resolve_model(options.model, &models, default_model.as_deref());
    let thinking_option = if omit_reasoning_effort {
        None
    } else {
        options.thinking_option_id.and_then(normalize_model_id)
    };
    let service_tier = if options.fast_mode && model_supports_fast(&model, &models) {
        Some("fast")
    } else {
        None
    };
    let permission = permission_preset(options.permission_mode);

    let thread_params = json!({
        "model": model,
        "cwd": options.cwd,
        "approvalPolicy": permission.approval_policy,
        "sandbox": permission.sandbox_mode,
        "serviceTier": service_tier,
        "experimentalRawEvents": false,
        "persistExtendedHistory": true,
    });
    let thread = client.request(
        "thread/start",
        thread_params,
        REQUEST_TIMEOUT,
        |method, params| {
            emit_notification(events, channel, ui_turn_id, method, params);
        },
    )?;
    let thread_id = thread
        .get("thread")
        .and_then(|thread| thread.get("id"))
        .and_then(Value::as_str)
        .ok_or_else(|| anyhow!("codex app-server did not return thread id"))?
        .to_string();

    client.request(
        "turn/start",
        turn_start_params(
            &thread_id,
            options.message,
            &permission,
            model.as_deref(),
            thinking_option.as_deref(),
            service_tier,
        ),
        REQUEST_TIMEOUT,
        |method, params| emit_notification(events, channel, ui_turn_id, method, params),
    )?;

    let mut pending_agent_messages: HashMap<String, String> = HashMap::new();
    let mut agent_message_order: Vec<String> = Vec::new();
    let mut completed_agent_messages: Vec<String> = Vec::new();
    let mut completed_agent_ids: HashSet<String> = HashSet::new();
    let mut tools: Vec<CapturedToolEvent> = Vec::new();
    let mut ordered_events: Vec<CapturedTurnEvent> = Vec::new();

    loop {
        if options.cancel.load(std::sync::atomic::Ordering::SeqCst) {
            let _ = client.request(
                "turn/interrupt",
                json!({"threadId": thread_id}),
                Duration::from_secs(5),
                |_, _| {},
            );
            bail!("turn canceled");
        }
        match client.next_message(TURN_TIMEOUT)? {
            AppServerLine::Stderr(line) => {
                events.emit(
                    channel.to_string(),
                    json!({"type": "thinking-delta", "turnId": ui_turn_id, "delta": format!("{line}\n")}),
                );
            }
            AppServerLine::Stdout(value) => {
                if value.get("id").is_some() && value.get("method").is_some() {
                    client.answer_server_request(&value)?;
                    continue;
                }
                let Some(method) = value.get("method").and_then(Value::as_str) else {
                    continue;
                };
                let params = value.get("params").unwrap_or(&Value::Null);
                match method {
                    "item/agentMessage/delta" => {
                        if let Some((item_id, delta)) = agent_message_delta(params) {
                            if !pending_agent_messages.contains_key(&item_id) {
                                agent_message_order.push(item_id.clone());
                            }
                            pending_agent_messages
                                .entry(item_id)
                                .or_default()
                                .push_str(&delta);
                            events.emit(
                                channel.to_string(),
                                json!({"type": "text-delta", "turnId": ui_turn_id, "delta": delta}),
                            );
                        }
                    }
                    "item/started" => {
                        if let Some(request) = capture_tool_request(params) {
                            events.emit(
                                channel.to_string(),
                                json!({
                                    "type": "tool-calls-requested",
                                    "turnId": ui_turn_id,
                                    "requests": [{
                                        "callId": request.call_id,
                                        "toolId": request.tool_id,
                                        "input": request.input,
                                    }]
                                }),
                            );
                        }
                    }
                    "turn/plan/updated" => {
                        if let Some(tool) = capture_plan_event(params, ui_turn_id) {
                            events.emit(
                                channel.to_string(),
                                json!({
                                    "type": "tool-invocations",
                                    "turnId": ui_turn_id,
                                    "invocations": [{
                                        "callId": tool.call_id,
                                        "toolId": tool.tool_id,
                                        "input": tool.input,
                                        "output": tool.output,
                                        "success": tool.success,
                                    }]
                                }),
                            );
                            tools.retain(|item| item.call_id != tool.call_id);
                            upsert_ordered_tool_event(&mut ordered_events, tool.clone());
                            tools.push(tool);
                        }
                    }
                    "item/completed" => {
                        if let Some(tool) = capture_tool_event(params) {
                            events.emit(
                                channel.to_string(),
                                json!({
                                    "type": "tool-invocations",
                                    "turnId": ui_turn_id,
                                    "invocations": [{
                                        "callId": tool.call_id,
                                        "toolId": tool.tool_id,
                                        "input": tool.input,
                                        "output": tool.output,
                                        "success": tool.success,
                                    }]
                                }),
                            );
                            upsert_ordered_tool_event(&mut ordered_events, tool.clone());
                            tools.push(tool);
                        } else if let Some((item_id, text)) = completed_agent_message(params) {
                            if completed_agent_ids.insert(item_id.clone()) {
                                let text = pending_agent_messages.remove(&item_id).unwrap_or(text);
                                if !text.trim().is_empty() {
                                    let text = text.trim().to_string();
                                    completed_agent_messages.push(text.clone());
                                    ordered_events.push(CapturedTurnEvent::Assistant(text));
                                }
                            }
                        }
                    }
                    "turn/completed" => {
                        let status = params
                            .get("turn")
                            .and_then(|turn| turn.get("status"))
                            .and_then(Value::as_str)
                            .unwrap_or("completed");
                        if status == "completed" {
                            break;
                        }
                        bail!("{}", turn_error_message(params));
                    }
                    "error" => bail!("{}", turn_error_message(params)),
                    _ => emit_notification(events, channel, ui_turn_id, method, params),
                }
            }
        }
    }

    for item_id in agent_message_order {
        if completed_agent_ids.contains(&item_id) {
            continue;
        }
        if let Some(text) = pending_agent_messages.remove(&item_id) {
            if !text.trim().is_empty() {
                let text = text.trim().to_string();
                completed_agent_messages.push(text.clone());
                ordered_events.push(CapturedTurnEvent::Assistant(text));
            }
        }
    }
    let assistant_text = completed_agent_messages.last().cloned().unwrap_or_default();

    Ok(CodexTurnOutcome {
        assistant_text,
        assistant_messages: completed_agent_messages,
        tools,
        events: ordered_events,
    })
}

fn turn_start_params(
    thread_id: &str,
    message: &str,
    permission: &PermissionPreset,
    model: Option<&str>,
    thinking_option: Option<&str>,
    service_tier: Option<&str>,
) -> Value {
    let mut params = serde_json::Map::new();
    params.insert("threadId".to_string(), json!(thread_id));
    params.insert(
        "input".to_string(),
        json!([{"type": "text", "text": message}]),
    );
    params.insert(
        "approvalPolicy".to_string(),
        json!(permission.approval_policy),
    );
    params.insert(
        "sandboxPolicy".to_string(),
        permission.sandbox_policy.clone(),
    );
    if let Some(model) = model.filter(|value| !value.trim().is_empty()) {
        params.insert("model".to_string(), json!(model));
    }
    if let Some(effort) = thinking_option.filter(|value| !value.trim().is_empty()) {
        params.insert("effort".to_string(), json!(effort));
    }
    if let Some(service_tier) = service_tier.filter(|value| !value.trim().is_empty()) {
        params.insert("serviceTier".to_string(), json!(service_tier));
    }
    Value::Object(params)
}

fn options_has_reasoning_effort(options: &CodexTurnOptions<'_>) -> bool {
    options
        .thinking_option_id
        .is_some_and(|value| !value.trim().is_empty() && value != "default")
}

fn is_openai_include_validation_error(message: &str) -> bool {
    let normalized = message.to_ascii_lowercase();
    normalized.contains("include[0]")
        && normalized.contains("invalid")
        && (normalized.contains("reasoning.encrypted_content")
            || normalized.contains("reasoning.encryptedcontent")
            || normalized.contains("rea...ent")
            || normalized.contains("supported values"))
}

fn upsert_ordered_tool_event(events: &mut Vec<CapturedTurnEvent>, tool: CapturedToolEvent) {
    if let Some(existing) = events.iter_mut().find(|event| match event {
        CapturedTurnEvent::Tool(existing_tool) => existing_tool.call_id == tool.call_id,
        CapturedTurnEvent::Assistant(_) => false,
    }) {
        *existing = CapturedTurnEvent::Tool(tool);
    } else {
        events.push(CapturedTurnEvent::Tool(tool));
    }
}

fn emit_notification(
    events: &EventEmitter,
    channel: &str,
    ui_turn_id: &str,
    method: &str,
    params: &Value,
) {
    match method {
        "stderr" => {
            if let Some(line) = params.get("line").and_then(Value::as_str) {
                events.emit(
                    channel.to_string(),
                    json!({"type": "thinking-delta", "turnId": ui_turn_id, "delta": format!("{line}\n")}),
                );
            }
        }
        "item/reasoning/summaryTextDelta" | "item/reasoning/textDelta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                events.emit(
                    channel.to_string(),
                    json!({"type": "thinking-delta", "turnId": ui_turn_id, "delta": delta}),
                );
            }
        }
        "item/commandExecution/outputDelta" | "command/exec/outputDelta" => {
            if let Some(delta) = params.get("delta").and_then(Value::as_str) {
                events.emit(
                    channel.to_string(),
                    json!({"type": "thinking-delta", "turnId": ui_turn_id, "delta": delta}),
                );
            }
        }
        _ => {}
    }
}

fn fetch_models_and_default(client: &mut AppServerClient) -> Result<(Vec<Value>, Option<String>)> {
    let response = client.request("model/list", json!({}), REQUEST_TIMEOUT, |_, _| {})?;
    let models = response
        .get("data")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .filter(|model| {
            !model
                .get("hidden")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .collect::<Vec<_>>();
    let configured = read_codex_configured_defaults(client);
    let configured_model = configured
        .model
        .filter(|model| model_exists(&models, model));
    let app_server_default = models
        .iter()
        .find(|model| {
            model
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .and_then(|model| model.get("id").and_then(Value::as_str))
        .map(str::to_string);
    let default_model = configured_model.or(app_server_default).or_else(|| {
        models
            .first()
            .and_then(|model| model.get("id").and_then(Value::as_str))
            .map(str::to_string)
    });
    Ok((models, default_model))
}

fn read_codex_configured_defaults(client: &mut AppServerClient) -> CodexConfiguredDefaults {
    let mut saved = CodexConfiguredDefaults::default();
    if let Ok(response) =
        client.request("getUserSavedConfig", json!({}), REQUEST_TIMEOUT, |_, _| {})
    {
        saved.model = response
            .get("config")
            .and_then(|config| config.get("model"))
            .and_then(Value::as_str)
            .and_then(normalize_model_id);
    }
    if saved.model.is_some() {
        return saved;
    }
    let mut config_read = CodexConfiguredDefaults::default();
    if let Ok(response) = client.request("config/read", json!({}), REQUEST_TIMEOUT, |_, _| {}) {
        config_read.model = response
            .get("config")
            .and_then(|config| config.get("model"))
            .and_then(Value::as_str)
            .and_then(normalize_model_id);
    }
    CodexConfiguredDefaults {
        model: saved.model.or(config_read.model),
    }
}

fn normalize_model_id(value: &str) -> Option<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn model_exists(models: &[Value], model_id: &str) -> bool {
    models
        .iter()
        .any(|model| model.get("id").and_then(Value::as_str) == Some(model_id))
}

fn model_is_effective_default(model: &Value, default_model: Option<&str>) -> bool {
    let id = model.get("id").and_then(Value::as_str);
    if let Some(default_model) = default_model {
        return id == Some(default_model);
    }
    model
        .get("isDefault")
        .and_then(Value::as_bool)
        .unwrap_or(false)
}

fn model_to_desktop_model(model: Value, is_default: bool) -> Value {
    let id = model
        .get("id")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let display_name = model
        .get("displayName")
        .and_then(Value::as_str)
        .unwrap_or(&id)
        .to_string();
    let supports_reasoning = model
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|values| !values.is_empty())
        .unwrap_or(false);
    let default_thinking_option = model
        .get("defaultReasoningEffort")
        .and_then(Value::as_str)
        .and_then(normalize_model_id);
    let thinking_options = model
        .get("supportedReasoningEfforts")
        .and_then(Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let id = entry
                        .get("reasoningEffort")
                        .or_else(|| entry.get("id"))
                        .and_then(Value::as_str)
                        .and_then(normalize_model_id)?;
                    let description = entry
                        .get("description")
                        .or_else(|| entry.get("summary"))
                        .and_then(Value::as_str)
                        .map(str::to_string);
                    let is_default = default_thinking_option.as_deref() == Some(id.as_str());
                    let label = id.clone();
                    Some(json!({
                        "id": id,
                        "label": label,
                        "description": description,
                        "isDefault": is_default,
                    }))
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    json!({
        "id": id,
        "displayName": display_name,
        "provider": "codex",
        "api": "cli",
        "contextWindow": 0,
        "maxOutputTokens": 0,
        "supportsReasoning": supports_reasoning,
        "isDefault": is_default,
        "thinkingOptions": thinking_options,
        "defaultThinkingOptionId": default_thinking_option,
    })
}

fn resolve_model(
    requested: Option<&str>,
    models: &[Value],
    default_model: Option<&str>,
) -> Option<String> {
    if let Some(requested) = requested.filter(|value| !value.trim().is_empty()) {
        if model_exists(models, requested) {
            return Some(requested.to_string());
        }
    }
    if let Some(default_model) = default_model.filter(|model| model_exists(models, model)) {
        return Some(default_model.to_string());
    }
    models
        .iter()
        .find(|model| {
            model
                .get("isDefault")
                .and_then(Value::as_bool)
                .unwrap_or(false)
        })
        .or_else(|| models.first())
        .and_then(|model| model.get("id").and_then(Value::as_str))
        .map(str::to_string)
}

fn model_supports_fast(model: &Option<String>, models: &[Value]) -> bool {
    let Some(model_id) = model.as_deref() else {
        return false;
    };
    models
        .iter()
        .find(|model| model.get("id").and_then(Value::as_str) == Some(model_id))
        .and_then(|model| model.get("additionalSpeedTiers"))
        .and_then(Value::as_array)
        .map(|tiers| tiers.iter().any(|tier| tier.as_str() == Some("fast")))
        .unwrap_or(false)
}

struct PermissionPreset {
    approval_policy: &'static str,
    sandbox_mode: &'static str,
    sandbox_policy: Value,
}

fn permission_preset(permission_mode: Option<&str>) -> PermissionPreset {
    match permission_mode.unwrap_or("workspace-write") {
        "read-only" => PermissionPreset {
            approval_policy: "on-request",
            sandbox_mode: "read-only",
            sandbox_policy: json!({"type": "readOnly"}),
        },
        "full-access" => PermissionPreset {
            approval_policy: "on-request",
            sandbox_mode: "danger-full-access",
            sandbox_policy: json!({"type": "dangerFullAccess"}),
        },
        _ => PermissionPreset {
            approval_policy: "on-request",
            sandbox_mode: "workspace-write",
            sandbox_policy: json!({"type": "workspaceWrite", "networkAccess": false}),
        },
    }
}

fn agent_message_delta(params: &Value) -> Option<(String, String)> {
    let delta = params.get("delta").and_then(Value::as_str)?.to_string();
    let item_id = params
        .get("itemId")
        .or_else(|| params.get("item_id"))
        .and_then(Value::as_str)
        .unwrap_or("__agent_message")
        .to_string();
    Some((item_id, delta))
}

fn completed_agent_message(params: &Value) -> Option<(String, String)> {
    let item = params.get("item")?;
    if item.get("type").and_then(Value::as_str) != Some("agentMessage") {
        return None;
    }
    let item_id = item
        .get("id")
        .or_else(|| item.get("itemId"))
        .and_then(Value::as_str)
        .unwrap_or("__agent_message")
        .to_string();
    let text = item.get("text").and_then(Value::as_str)?.trim().to_string();
    if text.is_empty() {
        None
    } else {
        Some((item_id, text))
    }
}

fn capture_tool_request(params: &Value) -> Option<CapturedToolEvent> {
    let item = params.get("item")?;
    let (tool_id, input) = tool_id_and_input(item)?;
    Some(CapturedToolEvent {
        call_id: item_id(item)?,
        tool_id,
        input: input.to_string(),
        output: String::new(),
        success: true,
    })
}

fn capture_plan_event(params: &Value, ui_turn_id: &str) -> Option<CapturedToolEvent> {
    if !params.get("plan").is_some_and(Value::is_array) {
        return None;
    }
    Some(CapturedToolEvent {
        call_id: format!("plan-{ui_turn_id}"),
        tool_id: "plan".to_string(),
        input: json!({
            "explanation": params.get("explanation").cloned().unwrap_or(Value::Null),
        })
        .to_string(),
        output: params.to_string(),
        success: true,
    })
}

fn capture_tool_event(params: &Value) -> Option<CapturedToolEvent> {
    let item = params.get("item")?;
    let (tool_id, input) = tool_id_and_input(item)?;
    Some(CapturedToolEvent {
        call_id: item_id(item)?,
        tool_id,
        input: input.to_string(),
        output: tool_output(item).to_string(),
        success: tool_success(item),
    })
}

fn item_id(item: &Value) -> Option<String> {
    item.get("id").and_then(Value::as_str).map(str::to_string)
}

fn tool_success(item: &Value) -> bool {
    !matches!(
        item.get("status").and_then(Value::as_str),
        Some("failed" | "declined" | "errored")
    ) && item.get("success").and_then(Value::as_bool) != Some(false)
}

fn tool_id_and_input(item: &Value) -> Option<(String, Value)> {
    let item_type = item.get("type").and_then(Value::as_str)?;
    match item_type {
        "commandExecution" => Some((
            "shell".to_string(),
            json!({
                "command": item.get("command").cloned().unwrap_or(Value::Null),
                "cwd": item.get("cwd").cloned().unwrap_or(Value::Null),
                "actions": item.get("commandActions").cloned().unwrap_or_else(|| json!([])),
                "source": item.get("source").cloned().unwrap_or(Value::Null),
            }),
        )),
        "fileChange" => Some((
            "apply_patch".to_string(),
            json!({
                "changes": item.get("changes").cloned().unwrap_or_else(|| json!([])),
                "files": file_change_paths(item),
            }),
        )),
        "mcpToolCall" => {
            let server = item.get("server").and_then(Value::as_str).unwrap_or("mcp");
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            Some((
                format!(
                    "mcp__{}__{}",
                    sanitize_tool_part(server),
                    sanitize_tool_part(tool)
                ),
                json!({
                    "server": server,
                    "tool": tool,
                    "arguments": item.get("arguments").cloned().unwrap_or(Value::Null),
                    "resourceUri": item.get("mcpAppResourceUri").cloned().unwrap_or(Value::Null),
                }),
            ))
        }
        "dynamicToolCall" => {
            let tool = item.get("tool").and_then(Value::as_str).unwrap_or("tool");
            let namespace = item.get("namespace").and_then(Value::as_str);
            let tool_id = namespace
                .filter(|value| !value.is_empty())
                .map(|namespace| format!("{}.{}", namespace, tool))
                .unwrap_or_else(|| tool.to_string());
            Some((
                tool_id,
                json!({
                    "namespace": namespace,
                    "tool": tool,
                    "arguments": item.get("arguments").cloned().unwrap_or(Value::Null),
                }),
            ))
        }
        "collabAgentToolCall" => Some((
            "sub_agent".to_string(),
            json!({
                "tool": item.get("tool").cloned().unwrap_or(Value::Null),
                "prompt": item.get("prompt").cloned().unwrap_or(Value::Null),
                "model": item.get("model").cloned().unwrap_or(Value::Null),
                "reasoningEffort": item.get("reasoningEffort").cloned().unwrap_or(Value::Null),
                "receiverThreadIds": item.get("receiverThreadIds").cloned().unwrap_or_else(|| json!([])),
            }),
        )),
        "webSearch" => Some((
            "web_search".to_string(),
            json!({
                "query": item.get("query").cloned().unwrap_or(Value::Null),
                "action": item.get("action").cloned().unwrap_or(Value::Null),
            }),
        )),
        "imageView" => Some((
            "view_image".to_string(),
            json!({
                "path": item.get("path").cloned().unwrap_or(Value::Null),
            }),
        )),
        "imageGeneration" => Some((
            "image_generation".to_string(),
            json!({
                "prompt": item.get("revisedPrompt").cloned().unwrap_or(Value::Null),
                "path": item.get("savedPath").cloned().unwrap_or(Value::Null),
            }),
        )),
        "plan" => Some((
            "plan".to_string(),
            json!({
                "text": item.get("text").cloned().unwrap_or(Value::Null),
            }),
        )),
        "reasoning" => Some((
            "thinking".to_string(),
            json!({
                "summary": item.get("summary").cloned().unwrap_or_else(|| json!([])),
            }),
        )),
        _ => None,
    }
}

fn tool_output(item: &Value) -> Value {
    match item.get("type").and_then(Value::as_str).unwrap_or_default() {
        "commandExecution" => json!({
            "stdout": item.get("aggregatedOutput").and_then(Value::as_str).unwrap_or_default(),
            "stderr": "",
            "exitCode": item.get("exitCode").cloned().unwrap_or(Value::Null),
            "durationMs": item.get("durationMs").cloned().unwrap_or(Value::Null),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
        }),
        "fileChange" => json!({
            "changes": item.get("changes").cloned().unwrap_or_else(|| json!([])),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
        }),
        "mcpToolCall" => json!({
            "result": item.get("result").cloned().unwrap_or(Value::Null),
            "error": item.get("error").cloned().unwrap_or(Value::Null),
            "durationMs": item.get("durationMs").cloned().unwrap_or(Value::Null),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
        }),
        "dynamicToolCall" => json!({
            "contentItems": item.get("contentItems").cloned().unwrap_or(Value::Null),
            "success": item.get("success").cloned().unwrap_or(Value::Null),
            "durationMs": item.get("durationMs").cloned().unwrap_or(Value::Null),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
        }),
        "collabAgentToolCall" => json!({
            "agentsStates": item.get("agentsStates").cloned().unwrap_or_else(|| json!({})),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
            "receiverThreadIds": item.get("receiverThreadIds").cloned().unwrap_or_else(|| json!([])),
        }),
        "webSearch" => json!({
            "query": item.get("query").cloned().unwrap_or(Value::Null),
            "action": item.get("action").cloned().unwrap_or(Value::Null),
        }),
        "imageView" => json!({
            "path": item.get("path").cloned().unwrap_or(Value::Null),
        }),
        "imageGeneration" => json!({
            "result": item.get("result").cloned().unwrap_or(Value::Null),
            "revisedPrompt": item.get("revisedPrompt").cloned().unwrap_or(Value::Null),
            "savedPath": item.get("savedPath").cloned().unwrap_or(Value::Null),
            "status": item.get("status").cloned().unwrap_or(Value::Null),
        }),
        "plan" => json!({
            "text": item.get("text").cloned().unwrap_or(Value::Null),
        }),
        "reasoning" => json!({
            "summary": item.get("summary").cloned().unwrap_or_else(|| json!([])),
            "content": item.get("content").cloned().unwrap_or_else(|| json!([])),
        }),
        _ => item.clone(),
    }
}

fn file_change_paths(item: &Value) -> Vec<String> {
    item.get("changes")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|change| {
            change
                .get("path")
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .collect()
}

fn sanitize_tool_part(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}

fn playwright_mcp_args_config(cdp_endpoint: Option<&str>) -> Result<String> {
    let mut args = vec!["--yes".to_string(), "@playwright/mcp@latest".to_string()];
    if let Some(endpoint) = cdp_endpoint
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        args.push(format!("--cdp-endpoint={endpoint}"));
    } else {
        args.push("--headless".to_string());
    }
    Ok(format!(
        "mcp_servers.playwright.args={}",
        serde_json::to_string(&args).context("encode Playwright MCP args")?
    ))
}

fn error_message(error: &Value) -> String {
    error
        .get("message")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| error.to_string())
}

fn turn_error_message(params: &Value) -> String {
    params
        .get("error")
        .and_then(|error| error.get("message").or(Some(error)))
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            params
                .get("turn")
                .and_then(|turn| turn.get("error"))
                .and_then(|error| error.get("message").or(Some(error)))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .unwrap_or_else(|| params.to_string())
}

#[cfg(test)]
mod tests {
    use super::{
        is_openai_include_validation_error, permission_preset, playwright_mcp_args_config,
        server_request_response, turn_start_params,
    };
    use serde_json::json;

    #[test]
    fn turn_start_params_omit_null_effort_and_service_tier() {
        let preset = permission_preset(None);
        let params = turn_start_params("thread-1", "hello", &preset, Some("gpt-5.4"), None, None);

        assert_eq!(params.get("threadId"), Some(&json!("thread-1")));
        assert_eq!(
            params.get("input"),
            Some(&json!([{"type": "text", "text": "hello"}]))
        );
        assert_eq!(params.get("model"), Some(&json!("gpt-5.4")));
        assert!(params.get("effort").is_none());
        assert!(params.get("serviceTier").is_none());
    }

    #[test]
    fn turn_start_params_include_selected_effort_and_fast_tier() {
        let preset = permission_preset(Some("full-access"));
        let params = turn_start_params(
            "thread-1",
            "hello",
            &preset,
            Some("gpt-5.4"),
            Some("low"),
            Some("fast"),
        );

        assert_eq!(params.get("effort"), Some(&json!("low")));
        assert_eq!(params.get("serviceTier"), Some(&json!("fast")));
        assert_eq!(
            params.get("sandboxPolicy"),
            Some(&json!({"type": "dangerFullAccess"}))
        );
    }

    #[test]
    fn openai_include_validation_errors_are_retryable_without_effort() {
        assert!(is_openai_include_validation_error(
            "request failed with status 400 Bad Request: {\"error\":{\"message\":\"Invalid value: 'rea...ent'. Supported values are: 'filesearchcall.results', 'reasoning.encryptedcontent'\"},\"param\":\"include[0]\"}"
        ));
        assert!(is_openai_include_validation_error(
            "Invalid value: 'reasoning.encrypted_content'. Supported values are: 'reasoning.encryptedcontent'. param include[0]"
        ));
        assert!(!is_openai_include_validation_error(
            "request failed with status 400 Bad Request: unrelated model error"
        ));
    }

    #[test]
    fn playwright_mcp_args_use_headless_without_cdp() {
        let config = playwright_mcp_args_config(None).unwrap();
        assert_eq!(
            config,
            "mcp_servers.playwright.args=[\"--yes\",\"@playwright/mcp@latest\",\"--headless\"]"
        );
    }

    #[test]
    fn playwright_mcp_args_use_cdp_endpoint_when_available() {
        let config = playwright_mcp_args_config(Some("http://127.0.0.1:9222")).unwrap();
        assert_eq!(
            config,
            "mcp_servers.playwright.args=[\"--yes\",\"@playwright/mcp@latest\",\"--cdp-endpoint=http://127.0.0.1:9222\"]"
        );
    }

    #[test]
    fn permissions_request_grants_requested_permissions_for_turn() {
        let response = server_request_response(
            "item/permissions/requestApproval",
            &json!({
                "permissions": {
                    "network": {"enabled": true},
                    "fileSystem": null,
                }
            }),
            "workspace-write",
        );
        assert_eq!(
            response,
            json!({
                "permissions": {
                    "network": {"enabled": true},
                },
                "scope": "turn",
            })
        );
    }

    #[test]
    fn permissions_request_denies_extra_permissions_in_read_only_mode() {
        let response = server_request_response(
            "item/permissions/requestApproval",
            &json!({
                "permissions": {
                    "network": {"enabled": true},
                    "fileSystem": {"read": ["/tmp"], "write": null},
                }
            }),
            "read-only",
        );
        assert_eq!(response, json!({"permissions": {}, "scope": "turn"}));
    }

    #[test]
    fn full_access_still_routes_mcp_approvals_to_the_app_server_client() {
        let preset = permission_preset(Some("full-access"));
        assert_eq!(preset.approval_policy, "on-request");
        assert_eq!(preset.sandbox_mode, "danger-full-access");
        assert_eq!(preset.sandbox_policy, json!({"type": "dangerFullAccess"}));
    }

    #[test]
    fn mcp_tool_elicitation_is_accepted_for_workspace_mode() {
        let response = server_request_response(
            "mcpServer/elicitation/request",
            &json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "serverName": "playwright",
                "mode": "form",
                "_meta": {
                    "codex_approval_kind": "mcp_tool_call",
                },
                "message": "Allow playwright to run tool?",
                "requestedSchema": {
                    "type": "object",
                },
            }),
            "workspace-write",
        );
        assert_eq!(
            response,
            json!({
                "action": "accept",
                "content": null,
                "_meta": {
                    "persist": "session",
                },
            })
        );
    }

    #[test]
    fn mcp_tool_elicitation_is_declined_in_read_only_mode() {
        let response = server_request_response(
            "mcpServer/elicitation/request",
            &json!({
                "threadId": "thread-1",
                "serverName": "playwright",
                "mode": "form",
                "_meta": {
                    "codex_approval_kind": "mcp_tool_call",
                },
                "message": "Allow playwright to run tool?",
                "requestedSchema": {
                    "type": "object",
                },
            }),
            "read-only",
        );
        assert_eq!(
            response,
            json!({
                "action": "decline",
                "content": null,
                "_meta": null,
            })
        );
    }

    #[test]
    fn non_tool_elicitation_is_cancelled_until_ui_can_render_it() {
        let response = server_request_response(
            "mcpServer/elicitation/request",
            &json!({
                "threadId": "thread-1",
                "serverName": "some-server",
                "mode": "form",
                "_meta": null,
                "message": "What should I do?",
                "requestedSchema": {
                    "type": "object",
                },
            }),
            "workspace-write",
        );
        assert_eq!(
            response,
            json!({
                "action": "cancel",
                "content": null,
                "_meta": null,
            })
        );
    }

    #[test]
    fn legacy_mcp_tool_user_input_is_answered_for_workspace_mode() {
        let response = server_request_response(
            "item/tool/requestUserInput",
            &json!({
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "call-1",
                "questions": [
                    {
                        "id": "mcp_tool_call_approval_call-1",
                        "header": "Approve app tool call?",
                        "question": "Allow playwright to run tool?",
                        "options": [
                            {
                                "label": "Allow",
                                "description": "Run the tool and continue."
                            },
                            {
                                "label": "Allow for this session",
                                "description": "Run the tool and remember this choice for this session."
                            },
                            {
                                "label": "Cancel",
                                "description": "Cancel this tool call."
                            }
                        ]
                    }
                ]
            }),
            "workspace-write",
        );
        assert_eq!(
            response,
            json!({
                "answers": {
                    "mcp_tool_call_approval_call-1": {
                        "answers": ["Allow for this session"],
                    },
                },
            })
        );
    }

    #[test]
    fn legacy_user_input_ignores_non_mcp_questions() {
        let response = server_request_response(
            "item/tool/requestUserInput",
            &json!({
                "questions": [
                    {
                        "id": "favorite_color",
                        "header": "Question",
                        "question": "Favorite color?",
                        "options": [
                            {
                                "label": "Blue",
                                "description": "Blue."
                            }
                        ]
                    }
                ]
            }),
            "workspace-write",
        );
        assert_eq!(response, json!({"answers": {}}));
    }
}
