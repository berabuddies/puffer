//! Monitor task text sanitization helpers.

/// Sanitizes a monitor task subject or description for user-facing display.
pub fn sanitize_monitor_task_text(value: &str) -> String {
    let without_prefix = strip_monitor_source_prefixes(value);
    let without_wrapping = unwrap_leading_message_quote(&without_prefix);
    let without_embedded_quote = unwrap_embedded_message_quote(&without_wrapping);
    let without_tail = trim_monitor_explanation_tail(&without_embedded_quote);
    let without_actor = strip_monitor_actor_phrases(&without_tail);
    let without_judgment = remove_monitor_judgment_sentences(&without_actor);
    let without_source_terms = remove_monitor_source_terms(&without_judgment);
    let with_plain_request_phrases = rewrite_monitor_request_phrases(&without_source_terms);
    let without_request_verb = strip_monitor_request_verbs(&with_plain_request_phrases);
    normalize_monitor_task_text(&without_request_verb)
}

/// Sanitizes monitor task subject and description fields in place.
pub fn sanitize_monitor_task_fields(subject: &mut String, description: &mut String) -> bool {
    let previous_subject = subject.clone();
    let previous_description = description.clone();
    *subject = sanitize_monitor_task_text(subject);
    *description = sanitize_monitor_task_text(description);
    *subject != previous_subject || *description != previous_description
}

fn strip_monitor_source_prefixes(value: &str) -> String {
    let mut text = value.trim();
    loop {
        let lowered = text.to_ascii_lowercase();
        if lowered.starts_with("chat_id") || lowered.starts_with("sender_id") {
            if let Some(colon_index) = text.find(':') {
                text = text[colon_index + 1..].trim();
                continue;
            }
        }
        let mut matched = None;
        for prefix in [
            "personal telegram message asks:",
            "personal telegram message ask:",
            "telegram message asks:",
            "telegram message ask:",
            "personal message asks:",
            "personal message ask:",
            "message asks:",
            "message ask:",
            "message says:",
            "message say:",
            "message says",
            "message say",
            "incoming group message asks:",
            "incoming group message ask:",
            "incoming group message mentions:",
            "incoming group message mention:",
            "incoming group message",
            "incoming dm asks in chinese:",
            "incoming dm asks:",
            "incoming dm ask:",
            "incoming dm",
            "group message asks:",
            "group message ask:",
            "group message mentions:",
            "group message mention:",
            "group message",
            "user asked in chinese:",
            "user asked:",
            "user asks:",
            "sender said in chinese:",
            "sender said:",
            "sender says:",
            "says:",
            "chat_id",
            "sender_id",
            "incoming telegram group message asks:",
            "incoming telegram group message ask:",
            "incoming telegram personal message asks:",
            "incoming telegram personal message ask:",
            "incoming telegram message asks:",
            "incoming telegram message ask:",
            "telegram group message asks:",
            "telegram group message ask:",
            "telegram message from",
            "incoming telegram message",
            "telegram user sent",
            "in telegram group",
            "message from chat_id",
        ] {
            if lowered.starts_with(prefix) {
                matched = Some(prefix.len());
                break;
            }
        }
        let Some(prefix_len) = matched else {
            return text.to_string();
        };
        text = text[prefix_len..]
            .trim_start_matches([' ', ':', '-', '>', '\t'])
            .trim();
    }
}

fn remove_monitor_judgment_sentences(value: &str) -> String {
    value
        .split_inclusive(['.', '!', '?'])
        .filter(|sentence| {
            let lowered = sentence.to_ascii_lowercase();
            !lowered.contains("actionable request")
                && !lowered.contains("actionable task")
                && !lowered.contains("non-actionable")
                && !lowered.contains("likely ")
                && !lowered.contains("triage")
                && !lowered.contains("need a reply")
                && !lowered.contains("needs a reply")
                && !lowered.contains("needs a response")
                && !lowered.contains("sender is asking")
                && !lowered.contains("sender asks")
                && !lowered.contains("message asks")
                && !lowered.contains("message says")
                && !lowered.contains("message is asking")
                && !lowered.contains("they are asking")
                && !lowered.contains("this looks like")
                && !lowered.contains("incoming group message")
                && !lowered.contains("incoming dm")
                && !lowered.contains("decide whether")
                && !lowered.contains("determine whether")
        })
        .collect::<String>()
}

fn remove_monitor_source_terms(value: &str) -> String {
    let mut text = value.to_string();
    for term in [
        "personal telegram",
        "telegram",
        "message asks",
        "message ask",
        "message says",
        "message say",
        "incoming group message",
        "incoming dm",
        "chat_id",
        "sender_id",
        "actionable request",
        "actionable task",
    ] {
        text = replace_ascii_case_insensitive(&text, term, "");
    }
    text
}

fn replace_ascii_case_insensitive(value: &str, needle: &str, replacement: &str) -> String {
    let mut text = value.to_string();
    loop {
        let lowered = text.to_ascii_lowercase();
        let Some(start) = lowered.find(needle) else {
            return text;
        };
        let end = start + needle.len();
        text.replace_range(start..end, replacement);
    }
}

fn unwrap_leading_message_quote(value: &str) -> String {
    for (open, close) in [('"', '"'), ('\'', '\''), ('“', '”'), ('‘', '’')] {
        if let Some(message) = unwrap_leading_message_quote_pair(value, open, close) {
            return message;
        }
    }
    value.trim().to_string()
}

fn unwrap_leading_message_quote_pair(value: &str, open: char, close: char) -> Option<String> {
    let text = value.trim();
    let rest = text.strip_prefix(open)?;
    let end = rest.find(close)?;
    let message = &rest[..end];
    let trailing = rest[end + close.len_utf8()..].trim();
    if trailing.is_empty() || looks_like_monitor_explanation(trailing) {
        return Some(message.to_string());
    }
    None
}

fn unwrap_embedded_message_quote(value: &str) -> String {
    for (open, close) in [('"', '"'), ('\'', '\''), ('“', '”'), ('‘', '’')] {
        if let Some(message) = unwrap_embedded_message_quote_pair(value, open, close) {
            return message;
        }
    }
    value.trim().to_string()
}

fn unwrap_embedded_message_quote_pair(value: &str, open: char, close: char) -> Option<String> {
    let open_index = value.find(open)?;
    let after_open = open_index + open.len_utf8();
    let Some(close_offset) = value[after_open..].find(close) else {
        let prefix = value[..open_index].to_ascii_lowercase();
        if looks_like_message_quote_prefix(&prefix) {
            return Some(value[after_open..].trim().to_string());
        }
        return None;
    };
    let close_index = after_open + close_offset;
    let prefix = value[..open_index].to_ascii_lowercase();
    if !looks_like_message_quote_prefix(&prefix) && !looks_like_source_or_judgment_text(value) {
        return None;
    }
    let trailing = value[close_index + close.len_utf8()..].trim();
    if trailing.is_empty() || looks_like_monitor_explanation(trailing) {
        return Some(value[after_open..close_index].trim().to_string());
    }
    None
}

fn looks_like_message_quote_prefix(value: &str) -> bool {
    [
        "message", "sender", "user", "dm", "group", "chat", "asks", "asked", "says", "said",
        "wrote", "mentions",
    ]
    .iter()
    .any(|term| value.contains(term))
}

fn looks_like_source_or_judgment_text(value: &str) -> bool {
    let lowered = value.to_ascii_lowercase();
    [
        "message",
        "sender",
        "user",
        "chat_id",
        "sender_id",
        "asked",
        "asks",
        "says",
        "said",
        "from ",
        "reply",
        "determine whether",
        "decide whether",
        "please ",
        "they are asking",
        "this looks like",
    ]
    .iter()
    .any(|term| lowered.contains(term))
}

fn looks_like_monitor_explanation(value: &str) -> bool {
    let lowered = value
        .trim_start_matches([' ', '.', ':', '-', '—', '\t'])
        .to_ascii_lowercase();
    lowered.starts_with("this is ")
        || lowered.starts_with("need ")
        || lowered.starts_with("needs ")
        || lowered.starts_with("sender ")
        || lowered.starts_with("likely ")
        || lowered.starts_with("decide whether ")
        || lowered.starts_with("determine whether ")
        || lowered.starts_with("please ")
        || lowered.starts_with("they are asking ")
        || lowered.starts_with("this looks like ")
        || lowered.starts_with("refers to")
        || lowered.starts_with("reply ")
        || lowered.starts_with("from ")
        || lowered.starts_with("in ")
        || lowered.starts_with("(")
}

fn trim_monitor_explanation_tail(value: &str) -> String {
    let mut text = value.trim().to_string();
    loop {
        let lowered = text.to_ascii_lowercase();
        let mut cut_index = None;
        for marker in [
            ". this looks like",
            ". they are asking",
            ". the sender",
            ". sender ",
            ". likely ",
            ". need ",
            ". needs ",
            ". please ",
            ". check ",
            ". remove ",
            ". determine whether",
            ". decide whether",
            " -- reply",
            " - reply",
            " — reply",
            " — please",
            " — determine",
            " — decide",
            ". decide ",
            ". determine ",
            " from chat_id",
            " from sender_id",
            "' from ",
        ] {
            if let Some(index) = lowered.find(marker) {
                cut_index = Some(cut_index.map_or(index, |current: usize| current.min(index)));
            }
        }
        let Some(index) = cut_index else {
            return text;
        };
        text.truncate(index);
        text = text.trim().to_string();
    }
}

fn strip_monitor_actor_phrases(value: &str) -> String {
    let mut text = value.trim();
    loop {
        let lowered = text.to_ascii_lowercase();
        let mut changed = false;
        for prefix in ["user ", "sender "] {
            if lowered.starts_with(prefix) {
                if let Some(index) = lowered.find(" reports that ") {
                    text = text[index + " reports that ".len()..].trim();
                    changed = true;
                    break;
                }
                if let Some(index) = lowered.find(" asks:") {
                    text = text[index + " asks:".len()..].trim();
                    changed = true;
                    break;
                }
                if let Some(index) = lowered.find(" says:") {
                    text = text[index + " says:".len()..].trim();
                    changed = true;
                    break;
                }
            }
        }
        if let Some(rest) = lowered.strip_prefix("the sender wants ") {
            let start = text.len() - rest.len();
            text = text[start..].trim();
            continue;
        }
        if lowered.starts_with("group ") {
            if let Some(index) = text.find(':') {
                text = text[index + 1..].trim();
                continue;
            }
            if let Some(index) = lowered.find(" whether ") {
                text = text[index + 1..].trim();
                continue;
            }
        }
        if let Some(index) = lowered.find(" says they ") {
            text = text[index + " says they ".len()..].trim();
            continue;
        }
        if let Some(index) = lowered.find(" says ") {
            text = text[index + " says ".len()..].trim();
            continue;
        }
        if let Some(index) = lowered.find("they feel ") {
            text = text[index + "they feel ".len()..].trim();
            continue;
        }
        if changed {
            continue;
        }
        if let Some(index) = lowered.find(" was asked:") {
            text = text[index + " was asked:".len()..].trim();
            continue;
        }
        return text.to_string();
    }
}

fn strip_monitor_request_verbs(value: &str) -> String {
    let mut text = value.trim();
    loop {
        let lowered = text.to_ascii_lowercase();
        let mut matched = None;
        for prefix in [
            "asks to ",
            "ask to ",
            "asking to ",
            "in asking to ",
            "asked to ",
            "wants to ",
            "want to ",
            "requests to ",
            "request to ",
            "to ",
        ] {
            if lowered.starts_with(prefix) {
                matched = Some(prefix.len());
                break;
            }
        }
        let Some(prefix_len) = matched else {
            return text.to_string();
        };
        text = text[prefix_len..].trim();
    }
}

fn rewrite_monitor_request_phrases(value: &str) -> String {
    let text = replace_ascii_case_insensitive(value, " and asks to ", " and wants to ");
    replace_ascii_case_insensitive(&text, " and requests to ", " and wants to ")
}

fn normalize_monitor_task_text(value: &str) -> String {
    let mut out = value.split_whitespace().collect::<Vec<_>>().join(" ");
    for (from, to) in [
        (" :", ":"),
        (" .", "."),
        (" ,", ","),
        (" !", "!"),
        (" ?", "?"),
        ("\".", "."),
        ("\" .", "."),
        ("  ", " "),
    ] {
        out = out.replace(from, to);
    }
    out.trim_matches([' ', ':', '-', '.', '"', '\''])
        .trim()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::sanitize_monitor_task_text;

    #[test]
    fn sanitizer_prefers_original_message_quotes() {
        for (input, expected) in [
            (
                "Incoming group message mentions @offline_owl and asks \"微信咋样了\". Likely needs a reply.",
                "微信咋样了",
            ),
            (
                "says: '这种问题不好复现' (\"This kind of problem is hard to reproduce\"). Review the surrounding conversation.",
                "这种问题不好复现",
            ),
            (
                "Incoming DM asks in Chinese: \"你这用的啥？自动回复？\" from chat_id 1751536985. Decide a short reply.",
                "你这用的啥？自动回复？",
            ),
            (
                "user asked in group Puffer: \"为什么puffer选择用CEF而不是MCP插件（类似Claude in chrome）?\" Need a concise technical explanation.",
                "为什么puffer选择用CEF而不是MCP插件（类似Claude in chrome）?",
            ),
            (
                "chat_id 5243256069, sender_id 1124596543: '@imcfs codex 报错：stream disconnected before completion: stream closed before response.completed'",
                "@imcfs codex 报错：stream disconnected before completion: stream closed before response.completed",
            ),
            (
                "mentions @offline_owl and asks \"微信咋样了",
                "微信咋样了",
            ),
            (
                "“这里交互改成，task的后续跟进另开一个session吧？” This looks like a request to change the workflow for task follow-up.",
                "这里交互改成，task的后续跟进另开一个session吧？",
            ),
            (
                "Chaofan (@imcfs) asks: “在对现在哪些东西可以从你那边接手” — reply with the current items.",
                "在对现在哪些东西可以从你那边接手",
            ),
            (
                "Worldclaw GTM 策略决策, Rose was asked: \"Hi Rose, could you pls help confirm is it from the same users or different users?\"",
                "Hi Rose, could you pls help confirm is it from the same users or different users?",
            ),
            (
                "They are asking what 'Pro account' refers to. Draft a concise reply.",
                "Pro account",
            ),
            (
                "asks to read messages directly from the WeChat database inside Docker. Determine whether we should inspect the Dockerized WeChat DB.",
                "read messages directly from the WeChat database inside Docker",
            ),
            (
                "user ByteScribe reports that tg task creation worked yesterday but fails today and they are investigating the cause. Check the integration and recent changes for regressions",
                "tg task creation worked yesterday but fails today and they are investigating the cause",
            ),
            (
                "in asking to publish one activity today and keep promotion running for another month. Decide how to respond and whether to approve the campaign timing",
                "publish one activity today and keep promotion running for another month",
            ),
            (
                "8就行' from Chaofan in Puffer Internal",
                "8就行",
            ),
            (
                "The sender wants task previews to be easier to understand at a glance. Remove boilerplate prefixes like 'message from...'.",
                "task previews to be easier to understand at a glance",
            ),
            (
                "group Puffer: zooey (@hypotyposis, zooey@tomo.inc) says they can only submit changes but cannot merge PRs and asks to be pulled into the puffer repo",
                "can only submit changes but cannot merge PRs and wants to be pulled into the puffer repo",
            ),
            (
                "group @hzliu whether there will be new sub-APIs for usage limit management and time limit management",
                "whether there will be new sub-APIs for usage limit management and time limit management",
            ),
            (
                "It works now, but they feel this approach is not reliable",
                "this approach is not reliable",
            ),
        ] {
            assert_eq!(sanitize_monitor_task_text(input), expected);
        }
    }
}
