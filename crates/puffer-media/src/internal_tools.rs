/// Identifies supported media internal tool commands embedded in Bash events.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GeneratedMediaInternalCommandKind {
    Image,
    Video,
}

/// Extracts generated media result JSON from a successful Bash tool event.
pub fn generated_media_internal_bash_output(
    tool_id: &str,
    input: &str,
    output: &str,
) -> Option<(GeneratedMediaInternalCommandKind, serde_json::Value)> {
    if tool_id != "Bash" {
        return None;
    }
    let input = serde_json::from_str::<serde_json::Value>(input).ok()?;
    let command = input.get("command").and_then(serde_json::Value::as_str)?;
    let kind = generated_media_internal_command_kind(command)?;
    let output = serde_json::from_str::<serde_json::Value>(output).ok()?;
    let stdout = output.get("stdout").and_then(serde_json::Value::as_str)?;
    let value = serde_json::from_str::<serde_json::Value>(stdout).ok()?;
    Some((kind, value))
}

/// Resolves a supported generated media internal command from Bash command text.
pub fn generated_media_internal_command_kind(
    command: &str,
) -> Option<GeneratedMediaInternalCommandKind> {
    if has_unquoted_shell_control_operator(command) {
        return None;
    }
    let tokens = shell_words::split(command).ok()?;
    if tokens.is_empty() {
        return None;
    }

    match tokens.first().map(String::as_str) {
        Some("imagegen") => Some(GeneratedMediaInternalCommandKind::Image),
        Some("videogen") => Some(GeneratedMediaInternalCommandKind::Video),
        _ => None,
    }
}

fn has_unquoted_shell_control_operator(command: &str) -> bool {
    let mut quote = None;
    let mut escaped = false;

    for character in command.chars() {
        if escaped {
            escaped = false;
            continue;
        }

        match quote {
            Some('\'') => {
                if character == '\'' {
                    quote = None;
                }
            }
            Some('"') => match character {
                '\\' => escaped = true,
                '"' => quote = None,
                _ => {}
            },
            _ => match character {
                '\\' => escaped = true,
                '\'' | '"' => quote = Some(character),
                ';' | '|' | '&' => return true,
                _ => {}
            },
        }
    }

    false
}
