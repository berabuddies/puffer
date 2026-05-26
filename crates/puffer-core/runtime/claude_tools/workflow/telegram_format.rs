use serde_json::Value;
use std::fmt::Write as _;

/// Formats a compact Telegram message-search event for terminal display.
pub(super) fn format_succinct_message_search(payload: &Value) -> String {
    let chat = payload.get("chat").unwrap_or(&Value::Null);
    let chat_title = value_string(chat, "title")
        .or_else(|| value_string(chat, "id"))
        .unwrap_or_else(|| "unknown chat".to_string());
    let chat_handle = value_string(chat, "handle")
        .filter(|value| !value.is_empty())
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    let query = value_string(payload, "query").unwrap_or_else(|| "<empty query>".to_string());
    let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
    let limit_reached = payload
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut output = String::new();
    let _ = writeln!(
        output,
        "Telegram search: \"{}\" in {}{}",
        one_line(&query),
        chat_title,
        chat_handle
    );
    let _ = writeln!(
        output,
        "{} match{} returned{}",
        count,
        if count == 1 { "" } else { "es" },
        if limit_reached {
            " (limit reached)"
        } else {
            ""
        }
    );

    let Some(results) = payload.get("results").and_then(Value::as_array) else {
        return trim_trailing_newline(output);
    };
    if results.is_empty() {
        return trim_trailing_newline(output);
    }

    for (index, result) in results.iter().enumerate() {
        let _ = writeln!(output);
        if results.len() > 1 {
            let _ = writeln!(output, "Match {}:", index + 1);
        } else {
            let _ = writeln!(output, "Match:");
        }

        match result.get("context").and_then(Value::as_array) {
            Some(context) if !context.is_empty() => {
                for message in context {
                    write_succinct_message_line(&mut output, message);
                }
            }
            _ => {
                if let Some(message) = result
                    .get("match")
                    .or_else(|| result.get("message"))
                    .filter(|value| value.is_object())
                {
                    write_succinct_message_line(&mut output, message);
                }
            }
        }

        if let Some(error) = value_string(result, "context_error").filter(|value| !value.is_empty())
        {
            let _ = writeln!(output, "context warning: {}", one_line(&error));
        }
    }

    trim_trailing_newline(output)
}

/// Formats a compact Telegram message-list event for terminal display.
pub(super) fn format_succinct_message_list(payload: &Value) -> String {
    let chat = payload.get("chat").unwrap_or(&Value::Null);
    let chat_title = value_string(chat, "title")
        .or_else(|| value_string(chat, "id"))
        .unwrap_or_else(|| "unknown chat".to_string());
    let chat_handle = value_string(chat, "handle")
        .filter(|value| !value.is_empty())
        .map(|value| format!(" ({value})"))
        .unwrap_or_default();
    let count = payload.get("count").and_then(Value::as_u64).unwrap_or(0);
    let limit_reached = payload
        .get("limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false);
    let sender_filter = value_string(payload, "sender_filter").filter(|value| !value.is_empty());
    let scanned = payload.get("scanned").and_then(Value::as_u64);
    let scan_limit = payload.get("scan_limit").and_then(Value::as_u64);
    let scan_limit_reached = payload
        .get("scan_limit_reached")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    let mut output = String::new();
    let _ = writeln!(output, "Telegram messages in {}{}", chat_title, chat_handle);
    let show_scan_metadata = sender_filter.is_some();
    if let Some(sender) = sender_filter {
        let _ = writeln!(output, "Sender filter: {}", one_line(&sender));
    }
    let _ = writeln!(
        output,
        "{} message{} returned{}",
        count,
        if count == 1 { "" } else { "s" },
        if limit_reached {
            " (limit reached)"
        } else {
            ""
        }
    );
    if let (true, Some(scanned), Some(scan_limit)) = (show_scan_metadata, scanned, scan_limit) {
        let _ = writeln!(
            output,
            "Scanned {scanned}/{scan_limit} message{}{}",
            if scan_limit == 1 { "" } else { "s" },
            if scan_limit_reached {
                " (scan limit reached)"
            } else {
                ""
            }
        );
    }
    if let Some(before_id) = payload.get("next_before_id").and_then(Value::as_i64) {
        let _ = writeln!(output, "Older page cursor: --before-id {before_id}");
    }

    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return trim_trailing_newline(output);
    };
    if messages.is_empty() {
        return trim_trailing_newline(output);
    }

    for message in messages {
        write_succinct_list_message_line(&mut output, message);
    }

    trim_trailing_newline(output)
}

fn write_succinct_message_line(output: &mut String, message: &Value) {
    let offset = message.get("offset").and_then(Value::as_i64);
    let offset = match offset {
        Some(value) if value > 0 => format!("+{value}"),
        Some(value) => value.to_string(),
        None if message
            .get("is_match")
            .and_then(Value::as_bool)
            .unwrap_or(false) =>
        {
            "0".to_string()
        }
        None => "?".to_string(),
    };
    let from = value_string(message, "from")
        .or_else(|| {
            message
                .get("sender")
                .and_then(|sender| value_string(sender, "title"))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let text = value_string(message, "text")
        .map(|value| one_line(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "[non-text message]".to_string());
    let _ = writeln!(output, "{offset:>3} {from}: {text}");
}

fn write_succinct_list_message_line(output: &mut String, message: &Value) {
    let id = message
        .get("id")
        .and_then(Value::as_i64)
        .map(|value| format!("#{value}"))
        .unwrap_or_else(|| "#?".to_string());
    let date = value_string(message, "date").unwrap_or_else(|| "unknown-date".to_string());
    let from = value_string(message, "from")
        .or_else(|| {
            message
                .get("sender")
                .and_then(|sender| value_string(sender, "title"))
        })
        .unwrap_or_else(|| "unknown".to_string());
    let text = value_string(message, "text")
        .map(|value| one_line(&value))
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| "[non-text message]".to_string());
    let _ = writeln!(output, "{id} {} {from}: {text}", one_line(&date));
}

fn value_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn one_line(value: &str) -> String {
    value.replace('\r', "\\r").replace('\n', "\\n")
}

fn trim_trailing_newline(mut output: String) -> String {
    while output.ends_with('\n') {
        output.pop();
    }
    output
}
