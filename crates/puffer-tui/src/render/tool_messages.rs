use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use serde_json::Value;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

#[cfg(test)]
mod tests;
mod user_question_tool;

const ORANGE_ACCENT: Color = Color::Indexed(214);
const TOOL_OUTPUT_COLOR: Color = Color::DarkGray;
const RUNNING_PULSE_MS: u128 = 800;
const DISPLAY_TAB_STOP: usize = 4;

// JSON syntax coloring.
const JSON_KEY_COLOR: Color = Color::Cyan;
const JSON_STRING_COLOR: Color = Color::Green;
const JSON_NUMBER_COLOR: Color = Color::Yellow;
const JSON_KEYWORD_COLOR: Color = Color::Magenta;

pub(super) fn render_tool_message(
    text: &str,
    expanded: bool,
    pulse_running: bool,
) -> Option<Vec<Line<'static>>> {
    let parsed = parse_tool_message(text)?;
    let mut lines = vec![Line::from(header_spans(&parsed, pulse_running))];

    let dim = Style::default()
        .fg(TOOL_OUTPUT_COLOR)
        .add_modifier(Modifier::DIM);
    let (preview_lines, is_json) = output_display_lines(parsed.tool_id, parsed.output, expanded);
    for (index, line) in preview_lines.into_iter().enumerate() {
        let prefix = if index == 0 { "└ " } else { "  " };
        let prefix_span = Span::styled(prefix.to_string(), dim);
        if is_json && !line.starts_with("[+") {
            let mut spans = vec![prefix_span];
            spans.extend(colorize_json_line(&line));
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(vec![prefix_span, Span::styled(line, dim)]));
        }
    }
    Some(lines)
}

struct ParsedToolMessage<'a> {
    tool_id: &'a str,
    status: &'a str,
    input: &'a str,
    output: Option<&'a str>,
}

fn parse_tool_message(text: &str) -> Option<ParsedToolMessage<'_>> {
    let (header, rest) = text.split_once('\n')?;
    let header = header.strip_prefix("Tool ")?;
    let (tool_id, status) = header.rsplit_once(" [")?;
    let status = status.strip_suffix(']')?;
    let input = rest.strip_prefix("input: ")?;
    let (input, output) = input
        .split_once('\n')
        .map(|(input, output)| (input, Some(output)))
        .unwrap_or((input, None));
    Some(ParsedToolMessage {
        tool_id,
        status,
        input,
        output,
    })
}

fn header_spans(parsed: &ParsedToolMessage<'_>, pulse_running: bool) -> Vec<Span<'static>> {
    let (symbol, style) = status_indicator(parsed.status, pulse_running);
    let mut spans = vec![Span::styled(symbol.to_string(), style)];
    spans.push(Span::styled(
        friendly_tool_name(parsed.tool_id),
        Style::default()
            .fg(ORANGE_ACCENT)
            .add_modifier(Modifier::BOLD),
    ));
    if let Some(summary) = summarize_input(parsed.tool_id, parsed.input) {
        spans.push(Span::raw(" "));
        spans.push(Span::raw(summary));
    }
    spans
}

fn status_indicator(status: &str, pulse_running: bool) -> (&'static str, Style) {
    if pulse_running {
        if pulse_visible() {
            return (
                "● ",
                Style::default()
                    .fg(Color::White)
                    .add_modifier(Modifier::BOLD),
            );
        }
        return ("  ", Style::default());
    }

    match status {
        "ok" => (
            "● ",
            Style::default()
                .fg(Color::LightGreen)
                .add_modifier(Modifier::BOLD),
        ),
        _ => (
            "● ",
            Style::default()
                .fg(Color::LightRed)
                .add_modifier(Modifier::BOLD),
        ),
    }
}

fn pulse_visible() -> bool {
    let elapsed = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_else(|_| Duration::from_millis(0))
        .as_millis();
    elapsed % RUNNING_PULSE_MS < RUNNING_PULSE_MS / 2
}

fn friendly_tool_name(tool_id: &str) -> String {
    match normalized_tool_id(tool_id).as_str() {
        "bash" => "Bash".to_string(),
        "read" | "read_file" => "Read".to_string(),
        "write" | "write_file" => "Write".to_string(),
        "edit" | "replace_in_file" => "Edit".to_string(),
        "glob" => "Glob".to_string(),
        "search_text" | "grep" => "Grep".to_string(),
        "list_dir" => "List Directory".to_string(),
        "move_path" => "Move Path".to_string(),
        "remove_path" => "Remove Path".to_string(),
        "websearch" => "Web Search".to_string(),
        "webfetch" => "Web Fetch".to_string(),
        "notebookedit" => "Notebook Edit".to_string(),
        "listmcpresourcestool" => "List MCP Resources".to_string(),
        "readmcpresourcetool" => "Read MCP Resource".to_string(),
        "task" => "Task".to_string(),
        "taskcreate" => "Task Create".to_string(),
        "taskupdate" => "Task Update".to_string(),
        "tasklist" => "Task List".to_string(),
        "taskget" => "Task Get".to_string(),
        "taskstop" => "Task Stop".to_string(),
        "taskoutput" => "Task Output".to_string(),
        "agent" => "Agent".to_string(),
        "subscriptioncreate" => "Subscription Create".to_string(),
        "subscriptionlist" => "Subscription List".to_string(),
        "subscriptionpause" => "Subscription Pause".to_string(),
        "subscriptiondelete" => "Subscription Delete".to_string(),
        "subscriberscaffold" => "Subscriber Scaffold".to_string(),
        "subscriberinstall" => "Subscriber Install".to_string(),
        "subscriberlist" => "Subscriber List".to_string(),
        "telegram" => "Telegram".to_string(),
        "telegramimportdesktop" | "telegramimportlocal" => "Telegram Desktop Import".to_string(),
        "telegramloginqr" => "Telegram QR Login".to_string(),
        "telegramloginqrwait" => "Telegram QR Login (wait)".to_string(),
        "telegramloginstart" => "Telegram Login".to_string(),
        "telegramloginsubmitcode" => "Telegram Login (code)".to_string(),
        "telegramloginsubmitpassword" => "Telegram Login (2FA)".to_string(),
        "emailconfigure" => "Email Configure".to_string(),
        "askuserquestion" => "Ask User Question".to_string(),
        other => other.replace('_', " "),
    }
}

fn summarize_input(tool_id: &str, input: &str) -> Option<String> {
    let parsed = serde_json::from_str::<Value>(input).ok();
    let summary = match normalized_tool_id(tool_id).as_str() {
        "bash" => extract_string(parsed.as_ref(), &["command"])
            .map(normalize_inline_text)
            .or_else(|| Some(normalize_inline_text(input))),
        "read" | "read_file" | "write" | "write_file" | "edit" | "replace_in_file"
        | "remove_path" | "list_dir" => {
            extract_string(parsed.as_ref(), &["file_path", "path"]).map(normalize_inline_text)
        }
        "move_path" => {
            let from = extract_string(parsed.as_ref(), &["from"])?;
            let to = extract_string(parsed.as_ref(), &["to"])?;
            Some(format!("{from} -> {to}"))
        }
        "search_text" => {
            extract_string(parsed.as_ref(), &["query", "pattern"]).map(normalize_inline_text)
        }
        "glob" => extract_string(parsed.as_ref(), &["pattern"]).map(normalize_inline_text),
        "websearch" => extract_string(parsed.as_ref(), &["query"]).map(normalize_inline_text),
        "webfetch" => extract_string(parsed.as_ref(), &["url"]).map(normalize_inline_text),
        "notebookedit" => extract_string(parsed.as_ref(), &["notebook_path"])
            .map(normalize_inline_text)
            .or_else(|| extract_string(parsed.as_ref(), &["cell_id"]).map(normalize_inline_text)),
        "readmcpresourcetool" => {
            extract_string(parsed.as_ref(), &["uri"]).map(normalize_inline_text)
        }
        "listmcpresourcestool" => {
            extract_string(parsed.as_ref(), &["server"]).map(normalize_inline_text)
        }
        "taskcreate" => extract_string(parsed.as_ref(), &["subject"]).map(normalize_inline_text),
        "taskupdate" => extract_string(parsed.as_ref(), &["id"]).map(normalize_inline_text),
        "task" | "agent" => extract_string(parsed.as_ref(), &["prompt"]).map(normalize_inline_text),
        "subscriptioncreate" | "subscriptiondelete" | "subscriberscaffold"
        | "subscriberinstall" => {
            extract_string(parsed.as_ref(), &["id"]).map(normalize_inline_text)
        }
        "subscriptionpause" => {
            let id = extract_string(parsed.as_ref(), &["id"])?;
            let paused = parsed
                .as_ref()
                .and_then(|v| v.as_object())
                .and_then(|o| o.get("paused"))
                .and_then(|v| v.as_bool())
                .unwrap_or(true);
            Some(format!(
                "{id} ({})",
                if paused { "pause" } else { "resume" }
            ))
        }
        "telegram" => telegram_input_summary(parsed.as_ref()),
        "telegramloginstart" => {
            extract_string(parsed.as_ref(), &["phone"]).map(normalize_inline_text)
        }
        "telegramimportdesktop" | "telegramimportlocal" => {
            extract_string(parsed.as_ref(), &["path"])
                .map(normalize_inline_text)
                .or_else(|| Some("Telegram Desktop tdata".to_string()))
        }
        "telegramloginqr" => Some("approval URL".to_string()),
        "telegramloginqrwait" => Some("waiting for approval".to_string()),
        // The submitted code, local passcode, and password are deliberately
        // never rendered.
        "telegramloginsubmitcode" => Some("(code redacted)".to_string()),
        "telegramloginsubmitpassword" => Some("(password redacted)".to_string()),
        // Email configure surfaces the username only — credentials and
        // hosts stay out of the transcript header.
        "emailconfigure" => {
            extract_string(parsed.as_ref(), &["username"]).map(normalize_inline_text)
        }
        "askuserquestion" => user_question_tool::input_summary(parsed.as_ref()),
        _ => extract_first_string(parsed.as_ref()).map(normalize_inline_text),
    }?;
    Some(truncate(&summary, 140))
}

fn telegram_input_summary(value: Option<&Value>) -> Option<String> {
    let action = value?
        .as_object()?
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or_default();
    match action {
        "import_desktop" => extract_string(value, &["path"])
            .map(normalize_inline_text)
            .or_else(|| Some("Telegram Desktop tdata".to_string())),
        "list_peers" => Some(
            extract_string(value, &["peer_kind"])
                .map(|kind| format!("{kind} peers"))
                .unwrap_or_else(|| "peers".to_string()),
        ),
        "login_qr" => Some("approval URL".to_string()),
        "login_qr_wait" => Some("waiting for approval".to_string()),
        "login_start" => extract_string(value, &["phone"])
            .map(normalize_inline_text)
            .or_else(|| Some("phone login".to_string())),
        "login_submit_code" => Some("(code redacted)".to_string()),
        "login_submit_password" => Some("(password redacted)".to_string()),
        "search_messages" => {
            let query = extract_string(value, &["query"]).map(normalize_inline_text)?;
            let peer = extract_string(value, &["peer"]).map(normalize_inline_text)?;
            Some(format!("{query} in {peer}"))
        }
        "search_peers" => extract_string(value, &["query"])
            .map(normalize_inline_text)
            .or_else(|| Some("search peers".to_string())),
        other if !other.is_empty() => Some(other.replace('_', " ")),
        _ => Some("Telegram action".to_string()),
    }
}

fn normalized_tool_id(tool_id: &str) -> String {
    tool_id.to_ascii_lowercase()
}

fn normalize_inline_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn extract_string<'a>(value: Option<&'a Value>, keys: &[&str]) -> Option<&'a str> {
    let object = value?.as_object()?;
    keys.iter()
        .find_map(|key| object.get(*key).and_then(Value::as_str))
}

fn extract_first_string(value: Option<&Value>) -> Option<&str> {
    value?.as_object()?.values().find_map(Value::as_str)
}

fn output_display_lines(
    tool_id: &str,
    output: Option<&str>,
    expanded: bool,
) -> (Vec<String>, bool) {
    let text = output.unwrap_or_default().trim();
    if text.is_empty() {
        return (Vec::new(), false);
    }

    let (lines, is_json) = display_output_lines(tool_id, text);
    let lines = lines
        .into_iter()
        .map(|line| sanitize_display_line(&line))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        return (Vec::new(), false);
    }

    if expanded || lines.len() <= 3 {
        return (lines, is_json);
    }

    let hidden = lines.len().saturating_sub(3);
    (
        vec![
            truncate(lines.first().map(String::as_str).unwrap_or_default(), 120),
            format!("[+{hidden} lines · press Ctrl+O to expand]"),
            truncate(
                lines
                    .get(lines.len().saturating_sub(2))
                    .map(String::as_str)
                    .unwrap_or_default(),
                120,
            ),
            truncate(lines.last().map(String::as_str).unwrap_or_default(), 120),
        ],
        is_json,
    )
}

fn display_output_lines(tool_id: &str, output: &str) -> (Vec<String>, bool) {
    match normalized_tool_id(tool_id).as_str() {
        "bash" => {
            if let Some(lines) = bash_output_lines(output) {
                return (lines, false);
            }
        }
        "read" => {
            if let Some(lines) = read_output_lines(output) {
                return (lines, false);
            }
        }
        "write" => {
            if let Some(lines) = write_output_lines(output) {
                return (lines, false);
            }
        }
        "edit" | "replace_in_file" => {
            if let Some(lines) = edit_output_lines(output) {
                return (lines, false);
            }
        }
        "glob" => {
            if let Some(lines) = glob_output_lines(output) {
                return (lines, false);
            }
        }
        "webfetch" => {
            if let Some(lines) = web_fetch_output_lines(output) {
                return (lines, false);
            }
        }
        "notebookedit" => {
            if let Some(lines) = notebook_edit_output_lines(output) {
                return (lines, false);
            }
        }
        "listmcpresourcestool" => {
            if let Some(lines) = list_mcp_resources_output_lines(output) {
                return (lines, false);
            }
        }
        "readmcpresourcetool" => {
            if let Some(lines) = read_mcp_resource_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriptioncreate" => {
            if let Some(lines) = subscription_create_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriptionlist" => {
            if let Some(lines) = subscription_list_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriptionpause" => {
            if let Some(lines) = subscription_pause_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriptiondelete" => {
            if let Some(lines) = subscription_delete_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriberscaffold" => {
            if let Some(lines) = subscriber_scaffold_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriberinstall" => {
            if let Some(lines) = subscriber_install_output_lines(output) {
                return (lines, false);
            }
        }
        "subscriberlist" => {
            if let Some(lines) = subscriber_list_output_lines(output) {
                return (lines, false);
            }
        }
        "telegramimportlocal"
        | "telegramloginstart"
        | "telegramloginsubmitcode"
        | "telegramloginsubmitpassword"
        | "emailconfigure" => {
            if let Some(lines) = status_next_output_lines(output) {
                return (lines, false);
            }
        }
        "askuserquestion" => {
            if let Some(lines) = user_question_tool::output_lines(output) {
                return (lines, false);
            }
        }
        _ => {}
    }
    if let Some(lines) = generic_json_output_lines(output) {
        return (lines, false);
    }
    // If the output is valid JSON, pretty-print it and flag for syntax coloring.
    if let Ok(parsed) = serde_json::from_str::<Value>(output.trim()) {
        if let Ok(pretty) = serde_json::to_string_pretty(&parsed) {
            return (pretty.lines().map(str::to_string).collect(), true);
        }
    }
    (output.lines().map(str::to_string).collect(), false)
}

fn bash_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let stdout = parsed
        .get("stdout")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end();
    let stderr = parsed
        .get("stderr")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .trim_end();

    let mut lines = Vec::new();
    if !stdout.is_empty() {
        lines.extend(stdout.lines().map(str::to_string));
    }
    if !stderr.is_empty() {
        lines.extend(stderr.lines().map(str::to_string));
    }
    Some(lines)
}

fn read_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let kind = parsed.get("type").and_then(Value::as_str)?;
    let file = parsed.get("file")?;
    match kind {
        "text" => Some(
            file.get("content")?
                .as_str()?
                .lines()
                .map(str::to_string)
                .collect(),
        ),
        "notebook" => Some(vec![format!(
            "Notebook with {} cells",
            file.get("cells")
                .and_then(Value::as_array)
                .map(|cells| cells.len())
                .unwrap_or(0)
        )]),
        "image" => Some(vec![format!(
            "Image file ({} bytes)",
            file.get("originalSize")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )]),
        "pdf" => Some(vec![format!(
            "PDF file ({} bytes)",
            file.get("originalSize")
                .and_then(Value::as_u64)
                .unwrap_or(0)
        )]),
        "file_unchanged" => Some(vec!["File unchanged since last read".to_string()]),
        _ => None,
    }
}

fn write_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let content = parsed.get("content").and_then(Value::as_str)?;
    Some(content.lines().map(str::to_string).collect())
}

fn edit_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    if let Some(lines) = parsed
        .get("structuredPatch")
        .and_then(Value::as_array)
        .and_then(|patches| patches.first())
        .and_then(|patch| patch.get("lines"))
        .and_then(Value::as_array)
    {
        let rendered = lines
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect::<Vec<_>>();
        if !rendered.is_empty() {
            return Some(rendered);
        }
    }
    parsed
        .get("newString")
        .and_then(Value::as_str)
        .map(|text| text.lines().map(str::to_string).collect())
}

fn glob_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let filenames = parsed.get("filenames")?.as_array()?;
    if filenames.is_empty() {
        return Some(vec!["No matches".to_string()]);
    }
    Some(
        filenames
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect(),
    )
}

fn web_fetch_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    parsed
        .get("result")
        .and_then(Value::as_str)
        .map(|text| text.lines().map(str::to_string).collect())
}

fn notebook_edit_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    if let Some(error) = parsed.get("error").and_then(Value::as_str) {
        return Some(vec![error.to_string()]);
    }
    if let Some(source) = parsed.get("new_source").and_then(Value::as_str) {
        let lines = source.lines().map(str::to_string).collect::<Vec<_>>();
        if !lines.is_empty() {
            return Some(lines);
        }
    }
    Some(vec![format!(
        "Notebook {}",
        parsed
            .get("edit_mode")
            .and_then(Value::as_str)
            .unwrap_or("updated")
    )])
}

fn list_mcp_resources_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let items = parsed.as_array()?;
    Some(
        items
            .iter()
            .map(|item| {
                let name = item
                    .get("name")
                    .and_then(Value::as_str)
                    .unwrap_or("resource");
                let server = item
                    .get("server")
                    .and_then(Value::as_str)
                    .unwrap_or("server");
                format!("{name} ({server})")
            })
            .collect(),
    )
}

fn read_mcp_resource_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let contents = parsed.get("contents")?.as_array()?;
    let mut lines = Vec::new();
    for content in contents {
        if let Some(text) = content.get("text").and_then(Value::as_str) {
            lines.extend(text.lines().map(str::to_string));
            continue;
        }
        if let Some(path) = content.get("blobSavedTo").and_then(Value::as_str) {
            lines.push(format!("Blob saved to {path}"));
        }
    }
    if lines.is_empty() {
        None
    } else {
        Some(lines)
    }
}

fn subscription_create_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let id = parsed.get("id").and_then(Value::as_str)?;
    let topic = parsed
        .get("source_topic")
        .and_then(Value::as_str)
        .unwrap_or("?");
    let action_type = parsed
        .get("action")
        .and_then(|a| a.get("type"))
        .and_then(Value::as_str)
        .unwrap_or("?");
    let mut lines = vec![format!(
        "Created subscription {id} (topic={topic}, action={action_type})"
    )];
    if parsed
        .get("classify_prompt")
        .and_then(Value::as_str)
        .is_some()
    {
        lines.push("LLM judge enabled".to_string());
    }
    if let Some(prefilter_kind) = parsed
        .get("prefilter")
        .and_then(|p| p.get("type"))
        .and_then(Value::as_str)
    {
        lines.push(format!("prefilter: {prefilter_kind}"));
    }
    Some(lines)
}

fn subscription_list_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let subs = parsed.get("subscriptions").and_then(Value::as_array)?;
    let mut lines = Vec::new();
    if subs.is_empty() {
        lines.push("(no subscriptions)".to_string());
    } else {
        for sub in subs {
            let id = sub.get("id").and_then(Value::as_str).unwrap_or("?");
            let status = sub.get("status").and_then(Value::as_str).unwrap_or("?");
            let topic = sub
                .get("source_topic")
                .and_then(Value::as_str)
                .unwrap_or("?");
            let action_type = sub
                .get("action")
                .and_then(|a| a.get("type"))
                .and_then(Value::as_str)
                .unwrap_or("?");
            lines.push(format!("{id}  [{status}]  {topic} -> {action_type}"));
        }
    }
    if let Some(running) = parsed
        .get("running_subscribers")
        .and_then(Value::as_array)
        .filter(|r| !r.is_empty())
    {
        let names: Vec<&str> = running.iter().filter_map(Value::as_str).collect();
        lines.push(format!("running subscribers: {}", names.join(", ")));
    }
    Some(lines)
}

fn subscription_pause_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let id = parsed.get("id").and_then(Value::as_str)?;
    let status = parsed.get("status").and_then(Value::as_str).unwrap_or("?");
    Some(vec![format!("{id} -> {status}")])
}

fn subscription_delete_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let deleted = parsed.get("deleted").and_then(Value::as_str)?;
    Some(vec![format!("deleted {deleted}")])
}

fn subscriber_scaffold_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let dir = parsed.get("dir").and_then(Value::as_str)?;
    let mut lines = vec![format!("scaffolded {dir}")];
    if let Some(next) = parsed.get("next").and_then(Value::as_str) {
        lines.push(next.to_string());
    }
    Some(lines)
}

fn subscriber_install_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let id = parsed.get("id").and_then(Value::as_str)?;
    let topic = parsed.get("topic").and_then(Value::as_str).unwrap_or(id);
    let dir = parsed.get("dir").and_then(Value::as_str).unwrap_or("");
    let mut lines = vec![format!("started {id} (topic={topic})")];
    if !dir.is_empty() {
        lines.push(dir.to_string());
    }
    Some(lines)
}

fn subscriber_list_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let items = parsed.as_array()?;
    if items.is_empty() {
        return Some(vec!["(no subscribers discovered)".to_string()]);
    }
    Some(
        items
            .iter()
            .map(|item| {
                let id = item.get("id").and_then(Value::as_str).unwrap_or("?");
                let topic = item.get("topic").and_then(Value::as_str).unwrap_or("?");
                let source = item.get("source").and_then(Value::as_str).unwrap_or("?");
                let running = item
                    .get("running")
                    .and_then(Value::as_bool)
                    .unwrap_or(false);
                let state = if running { "running" } else { "stopped" };
                format!("{id}  [{state}]  ({source})  topic={topic}")
            })
            .collect(),
    )
}

fn status_next_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    let mut lines = Vec::new();
    if let Some(status) = parsed.get("status").and_then(Value::as_str) {
        lines.push(format!("status: {status}"));
    }
    if let Some(next) = parsed.get("next").and_then(Value::as_str) {
        lines.push(next.to_string());
    }
    if lines.is_empty() {
        return None;
    }
    Some(lines)
}

fn generic_json_output_lines(output: &str) -> Option<Vec<String>> {
    let parsed = serde_json::from_str::<Value>(output).ok()?;
    if let Some(text) = parsed.get("result").and_then(Value::as_str) {
        return Some(text.lines().map(str::to_string).collect());
    }
    if let Some(text) = parsed.get("content").and_then(Value::as_str) {
        return Some(text.lines().map(str::to_string).collect());
    }
    None
}

/// Colorizes a single line of JSON output into styled spans.
fn colorize_json_line(line: &str) -> Vec<Span<'static>> {
    let punct_style = Style::default()
        .fg(TOOL_OUTPUT_COLOR)
        .add_modifier(Modifier::DIM);
    let key_style = Style::default().fg(JSON_KEY_COLOR);
    let string_style = Style::default().fg(JSON_STRING_COLOR);
    let number_style = Style::default().fg(JSON_NUMBER_COLOR);
    let keyword_style = Style::default().fg(JSON_KEYWORD_COLOR);

    let bytes = line.as_bytes();
    let len = bytes.len();
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut i = 0;

    // Leading whitespace.
    let ws_start = i;
    while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
        i += 1;
    }
    if i > ws_start {
        spans.push(Span::raw(line[ws_start..i].to_string()));
    }

    while i < len {
        match bytes[i] {
            b'"' => {
                let start = i;
                i += 1;
                while i < len && bytes[i] != b'"' {
                    if bytes[i] == b'\\' {
                        i += 1;
                    }
                    i += 1;
                }
                if i < len {
                    i += 1; // closing quote
                }
                let s = line[start..i].to_string();
                // Look ahead past whitespace for ':' to distinguish keys from values.
                let mut j = i;
                while j < len && bytes[j] == b' ' {
                    j += 1;
                }
                if j < len && bytes[j] == b':' {
                    spans.push(Span::styled(s, key_style));
                } else {
                    spans.push(Span::styled(s, string_style));
                }
            }
            b'0'..=b'9' => {
                let start = i;
                while i < len
                    && (bytes[i].is_ascii_digit()
                        || bytes[i] == b'.'
                        || bytes[i] == b'e'
                        || bytes[i] == b'E'
                        || bytes[i] == b'+'
                        || bytes[i] == b'-')
                {
                    i += 1;
                }
                spans.push(Span::styled(line[start..i].to_string(), number_style));
            }
            b'-' if i + 1 < len && bytes[i + 1].is_ascii_digit() => {
                let start = i;
                i += 1;
                while i < len
                    && (bytes[i].is_ascii_digit()
                        || bytes[i] == b'.'
                        || bytes[i] == b'e'
                        || bytes[i] == b'E')
                {
                    i += 1;
                }
                spans.push(Span::styled(line[start..i].to_string(), number_style));
            }
            b't' if line.get(i..i + 4) == Some("true") => {
                spans.push(Span::styled("true".to_string(), keyword_style));
                i += 4;
            }
            b'f' if line.get(i..i + 5) == Some("false") => {
                spans.push(Span::styled("false".to_string(), keyword_style));
                i += 5;
            }
            b'n' if line.get(i..i + 4) == Some("null") => {
                spans.push(Span::styled("null".to_string(), keyword_style));
                i += 4;
            }
            b'{' | b'}' | b'[' | b']' | b':' | b',' => {
                spans.push(Span::styled(String::from(bytes[i] as char), punct_style));
                i += 1;
            }
            b' ' | b'\t' => {
                let start = i;
                while i < len && (bytes[i] == b' ' || bytes[i] == b'\t') {
                    i += 1;
                }
                spans.push(Span::raw(line[start..i].to_string()));
            }
            _ => {
                // Handle non-ASCII (UTF-8 multibyte) gracefully.
                if let Some(ch) = line[i..].chars().next() {
                    spans.push(Span::styled(ch.to_string(), punct_style));
                    i += ch.len_utf8();
                } else {
                    i += 1;
                }
            }
        }
    }

    if spans.is_empty() {
        spans.push(Span::raw(line.to_string()));
    }
    spans
}

fn truncate(text: &str, max_chars: usize) -> String {
    let chars = text.chars().collect::<Vec<_>>();
    if chars.len() <= max_chars {
        return text.to_string();
    }
    let retained = chars
        .into_iter()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    format!("{retained}…")
}

fn sanitize_display_line(text: &str) -> String {
    let mut rendered = String::with_capacity(text.len());
    let mut column = 0usize;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\t' => {
                let spaces = DISPLAY_TAB_STOP - (column % DISPLAY_TAB_STOP);
                rendered.push_str(&" ".repeat(spaces));
                column += spaces;
            }
            '\u{1b}' => {
                if chars.next_if_eq(&'[').is_some() {
                    while let Some(next) = chars.next() {
                        if ('@'..='~').contains(&next) {
                            break;
                        }
                    }
                }
            }
            control if control.is_control() => {}
            _ => {
                rendered.push(ch);
                column += 1;
            }
        }
    }
    rendered
}
