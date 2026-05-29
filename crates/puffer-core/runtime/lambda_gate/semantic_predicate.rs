use serde_json::Value;
use std::path::Path;

#[derive(Clone, Copy)]
enum PredicateKind {
    AbsolutePath,
    ApiJson,
    ArxivId,
    AudioPath,
    BackendMode,
    Codec,
    EmergencyNumber,
    EvmAddress,
    FilePath,
    Format,
    Fps,
    GraphqlSuccess,
    Identifier,
    Iso8601WithTz,
    JsonField,
    KnownApp,
    Lat,
    Lng,
    LocalOrigin,
    LoopbackHost,
    MeetUrl,
    McpToolName,
    ParsedPaper,
    Path,
    PlanPath,
    Port,
    PositiveText,
    PrecisionMode,
    Provider,
    Rating,
    ScriptPath,
    Uri,
    UsageProvider,
    Url,
    YoutubeUrl,
}

/// Returns whether a semantic predicate expression is known to this runtime.
pub(super) fn is_supported_expr(expr: &str) -> bool {
    predicate_name(expr).and_then(predicate_kind).is_some()
}

/// Evaluates a semantic predicate expression against one JSON value.
pub(super) fn matches(value: &Value, expr: &str) -> Option<bool> {
    let name = predicate_name(expr)?;
    let kind = predicate_kind(name)?;
    Some(match kind {
        PredicateKind::AbsolutePath => value
            .as_str()
            .is_some_and(|text| Path::new(text.trim()).is_absolute()),
        PredicateKind::ApiJson => api_json(value),
        PredicateKind::ArxivId => value.as_str().is_some_and(valid_arxiv_id_list),
        PredicateKind::AudioPath => value.as_str().is_some_and(|text| {
            extension_in(
                text,
                &["aac", "aiff", "flac", "m4a", "mp3", "ogg", "opus", "wav"],
            )
        }),
        PredicateKind::BackendMode => value.as_str().is_some_and(accelerate_backend),
        PredicateKind::Codec => value.as_str().is_some_and(touchdesigner_codec),
        PredicateKind::EmergencyNumber => value.as_str().is_some_and(emergency_number),
        PredicateKind::EvmAddress => value.as_str().is_some_and(valid_evm_address),
        PredicateKind::FilePath => value.as_str().is_some_and(path_like),
        PredicateKind::Format => value
            .as_str()
            .is_some_and(|text| format_matches(name, text)),
        PredicateKind::Fps => json_integer(value).is_some_and(|fps| (1..=60).contains(&fps)),
        PredicateKind::GraphqlSuccess => graphql_success(value),
        PredicateKind::Identifier => value.as_str().is_some_and(identifier_like),
        PredicateKind::Iso8601WithTz => value.as_str().is_some_and(iso8601_with_tz),
        PredicateKind::JsonField => value.as_str().is_some_and(json_field_path),
        PredicateKind::KnownApp => value.as_str().is_some_and(known_deep_link_app),
        PredicateKind::Lat => {
            json_number(value).is_some_and(|number| (-90.0..=90.0).contains(&number))
        }
        PredicateKind::Lng => {
            json_number(value).is_some_and(|number| (-180.0..=180.0).contains(&number))
        }
        PredicateKind::LocalOrigin => value.as_str().is_some_and(local_origin_url),
        PredicateKind::LoopbackHost => value.as_str().is_some_and(loopback_host),
        PredicateKind::MeetUrl => value.as_str().is_some_and(meet_url),
        PredicateKind::McpToolName => value.as_str().is_some_and(normalized_mcp_tool_name),
        PredicateKind::ParsedPaper => parsed_paper_value(value),
        PredicateKind::Path => value.as_str().is_some_and(path_like),
        PredicateKind::PlanPath => value.as_str().is_some_and(plan_path),
        PredicateKind::Port => {
            json_integer(value).is_some_and(|port| (1024..=65535).contains(&port))
        }
        PredicateKind::PositiveText => value.as_str().is_some_and(positive_text),
        PredicateKind::PrecisionMode => value.as_str().is_some_and(accelerate_precision),
        PredicateKind::Provider => value
            .as_str()
            .is_some_and(|text| provider_matches(name, text)),
        PredicateKind::Rating => {
            json_number(value).is_some_and(|number| (0.0..=5.0).contains(&number))
        }
        PredicateKind::ScriptPath => value
            .as_str()
            .is_some_and(|text| extension_in(text, &["py"])),
        PredicateKind::Uri => value.as_str().is_some_and(valid_uri),
        PredicateKind::UsageProvider => value.as_str().is_some_and(model_usage_provider),
        PredicateKind::Url => value.as_str().is_some_and(valid_url),
        PredicateKind::YoutubeUrl => value.as_str().is_some_and(youtube_url),
    })
}

fn predicate_kind(name: &str) -> Option<PredicateKind> {
    Some(match name {
        "api_format" => PredicateKind::ApiJson,
        "is_abs_path" => PredicateKind::AbsolutePath,
        "codec_license_ok" => PredicateKind::Codec,
        "claude_history_path" => PredicateKind::Path,
        "dest_codex_home" => PredicateKind::Path,
        "emergency_number" => PredicateKind::EmergencyNumber,
        "ephemeral_port" => PredicateKind::Port,
        "formula_well_formed" | "is_query" | "is_text" | "nonempty_query" | "query_nonempty" => {
            PredicateKind::PositiveText
        }
        "fps_in_range" => PredicateKind::Fps,
        "gql_success" | "graphql_errors_checked" => PredicateKind::GraphqlSuccess,
        "is_audio" => PredicateKind::AudioPath,
        "known_app" => PredicateKind::KnownApp,
        "is_folder"
        | "output_dir_ready"
        | "output_excalidraw_path"
        | "output_path_ready"
        | "readable_folder"
        | "readable_inputs"
        | "writable_output" => PredicateKind::FilePath,
        "is_category"
        | "is_channel"
        | "is_chembl_id"
        | "is_compound"
        | "is_device"
        | "is_gene"
        | "is_jid"
        | "is_model"
        | "is_node"
        | "is_repo"
        | "is_session"
        | "is_sourced"
        | "is_target"
        | "is_template"
        | "is_title"
        | "is_trace"
        | "is_user"
        | "known_bio_category"
        | "known_clawbio_skill"
        | "known_template"
        | "model_override_valid"
        | "model_supported"
        | "short_action_label"
        | "supported_connector"
        | "system_skill"
        | "valid_condition_id"
        | "valid_content_filter"
        | "valid_favorite_action"
        | "valid_group_action"
        | "valid_id"
        | "valid_language"
        | "valid_library_kind"
        | "valid_list"
        | "valid_media_format"
        | "valid_member_action"
        | "valid_model"
        | "valid_place_id"
        | "valid_playback_read"
        | "valid_playback_write"
        | "valid_queue_action"
        | "valid_quality"
        | "valid_search_type"
        | "valid_smapi_category"
        | "valid_tags"
        | "valid_template"
        | "valid_token_id"
        | "valid_tunein_action" => PredicateKind::Identifier,
        "valid_uri" => PredicateKind::Uri,
        "is_url" | "valid_url" => PredicateKind::Url,
        "is_youtube" | "is_youtube_url" => PredicateKind::YoutubeUrl,
        "iso8601_with_tz" => PredicateKind::Iso8601WithTz,
        "json_format" | "pretty_format" | "valid_format" => PredicateKind::Format,
        "loopback_only" => PredicateKind::LoopbackHost,
        "meet_url_valid" => PredicateKind::MeetUrl,
        "tool_names_normalized" => PredicateKind::McpToolName,
        "origin_local" => PredicateKind::LocalOrigin,
        "plan_path" => PredicateKind::PlanPath,
        "is_pyscript" | "is_script" => PredicateKind::ScriptPath,
        "parsed_ok" => return Some(PredicateKind::ParsedPaper),
        "provider_bland" | "provider_twilio" | "provider_vapi" => PredicateKind::Provider,
        "real_prose" => PredicateKind::PositiveText,
        "valid_address" => PredicateKind::EvmAddress,
        "valid_arxiv_id" => PredicateKind::ArxivId,
        "valid_backend" => PredicateKind::BackendMode,
        "valid_date" | "valid_interval" => PredicateKind::PositiveText,
        "valid_items_path" | "valid_json_id" => PredicateKind::JsonField,
        "valid_lat" => PredicateKind::Lat,
        "valid_lng" => PredicateKind::Lng,
        "valid_precision" => PredicateKind::PrecisionMode,
        "valid_provider" => PredicateKind::UsageProvider,
        "valid_rating" => PredicateKind::Rating,
        _ => return None,
    })
}

fn predicate_name(expr: &str) -> Option<&str> {
    let (name, rest) = expr.trim().split_once('(')?;
    rest.trim().strip_suffix(')')?;
    let name = name.trim();
    is_identifier(name).then_some(name)
}

fn valid_arxiv_id_list(text: &str) -> bool {
    let mut seen = false;
    for item in text.split(',') {
        let id = item.trim();
        if id.is_empty() || !valid_arxiv_id(id) {
            return false;
        }
        seen = true;
    }
    seen
}

fn valid_arxiv_id(id: &str) -> bool {
    let base = id
        .rsplit_once('v')
        .and_then(|(prefix, suffix)| {
            (!suffix.is_empty() && suffix.chars().all(|ch| ch.is_ascii_digit())).then_some(prefix)
        })
        .unwrap_or(id);
    valid_new_arxiv_id(base) || valid_old_arxiv_id(base)
}

fn valid_new_arxiv_id(id: &str) -> bool {
    let Some((ym, number)) = id.split_once('.') else {
        return false;
    };
    ym.len() == 4
        && ym.chars().all(|ch| ch.is_ascii_digit())
        && matches!(number.len(), 4 | 5)
        && number.chars().all(|ch| ch.is_ascii_digit())
}

fn valid_old_arxiv_id(id: &str) -> bool {
    let Some((archive, number)) = id.split_once('/') else {
        return false;
    };
    !archive.is_empty()
        && archive
            .chars()
            .all(|ch| ch.is_ascii_alphabetic() || matches!(ch, '-' | '.'))
        && number.len() == 7
        && number.chars().all(|ch| ch.is_ascii_digit())
}

fn valid_evm_address(text: &str) -> bool {
    text.len() == 42 && text.starts_with("0x") && text[2..].chars().all(|ch| ch.is_ascii_hexdigit())
}

fn emergency_number(text: &str) -> bool {
    let digits = text
        .chars()
        .filter(|ch| ch.is_ascii_digit())
        .collect::<String>();
    matches!(
        digits.as_str(),
        "000" | "110" | "112" | "118" | "119" | "911" | "999"
    ) || digits.ends_with("911")
        || digits.ends_with("112")
}

fn valid_url(text: &str) -> bool {
    url::Url::parse(text)
        .ok()
        .is_some_and(|url| matches!(url.scheme(), "http" | "https") && url.host_str().is_some())
}

fn valid_uri(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || has_control(text) {
        return false;
    }
    if let Ok(value) = serde_json::from_str::<Value>(text) {
        return match value {
            Value::Array(items) => !items.is_empty() && items.iter().all(valid_uri_value),
            other => valid_uri_value(&other),
        };
    }
    valid_uri_text(text)
}

fn valid_uri_value(value: &Value) -> bool {
    value.as_str().is_some_and(valid_uri_text)
}

fn valid_uri_text(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || has_control(text) {
        return false;
    }
    if spotify_uri(text) || spotify_bare_id(text) {
        return true;
    }
    valid_url(text)
}

fn known_deep_link_app(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "codex"
            | "cursor"
            | "vscode"
            | "visualstudiocode"
            | "visualstudio"
            | "vscodeinsiders"
            | "visualstudiocodeinsiders"
            | "slack"
    )
}

fn model_usage_provider(text: &str) -> bool {
    matches!(normalized_token(text).as_str(), "codex" | "claude")
}

fn accelerate_precision(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "no" | "none" | "fp16" | "bf16" | "fp8"
    )
}

fn accelerate_backend(text: &str) -> bool {
    matches!(
        normalized_token(text).as_str(),
        "cpu"
            | "singlecpu"
            | "singlegpu"
            | "multigpu"
            | "ddp"
            | "deepspeed"
            | "fsdp"
            | "megatronlm"
            | "tpu"
            | "xla"
    )
}

fn touchdesigner_codec(text: &str) -> bool {
    matches!(normalized_token(text).as_str(), "prores" | "mjpa")
}

fn plan_path(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty() || has_control(text) || text.contains("..") || !text.ends_with(".md") {
        return false;
    }
    text.starts_with(".hermes/plans/") || text.contains("/.hermes/plans/")
}

fn normalized_token(text: &str) -> String {
    text.trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect()
}

fn spotify_uri(text: &str) -> bool {
    let mut parts = text.split(':');
    matches!(parts.next(), Some("spotify"))
        && matches!(
            parts.next(),
            Some("track" | "album" | "artist" | "playlist" | "show" | "episode")
        )
        && parts
            .next()
            .is_some_and(|id| !id.is_empty() && id.chars().all(|ch| ch.is_ascii_alphanumeric()))
        && parts.next().is_none()
}

fn spotify_bare_id(text: &str) -> bool {
    matches!(text.len(), 22 | 32)
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
}

fn youtube_url(text: &str) -> bool {
    parsed_host(text).is_some_and(|host| {
        host == "youtu.be" || host == "youtube.com" || host.ends_with(".youtube.com")
    })
}

fn meet_url(text: &str) -> bool {
    parsed_host(text).is_some_and(|host| host == "meet.google.com")
}

fn normalized_mcp_tool_name(text: &str) -> bool {
    let text = text.trim();
    if text.is_empty()
        || text.len() > 128
        || text.chars().any(char::is_control)
        || !text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
    {
        return false;
    }
    if let Some(stripped) = text.strip_prefix("mcp__") {
        let Some((server, tool)) = stripped.split_once("__") else {
            return false;
        };
        return !server.is_empty() && !tool.is_empty();
    }
    text.strip_prefix("mcp_")
        .and_then(|rest| rest.split_once('_'))
        .is_some_and(|(server, tool)| !server.is_empty() && !tool.is_empty())
}

fn local_origin_url(text: &str) -> bool {
    url::Url::parse(text)
        .ok()
        .and_then(|url| url.host_str().map(loopback_host))
        .unwrap_or(false)
}

fn parsed_host(text: &str) -> Option<String> {
    url::Url::parse(text)
        .ok()
        .and_then(|url| url.host_str().map(|host| host.to_ascii_lowercase()))
}

fn loopback_host(text: &str) -> bool {
    matches!(text, "localhost" | "127.0.0.1" | "::1" | "[::1]")
}

fn format_matches(name: &str, text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    match name {
        "json_format" => text == "json",
        "pretty_format" => text == "pretty",
        _ => matches!(
            text.as_str(),
            "api"
                | "csv"
                | "docx"
                | "gif"
                | "html"
                | "json"
                | "m4a"
                | "markdown"
                | "md"
                | "mp3"
                | "mp4"
                | "pdf"
                | "plain"
                | "pretty"
                | "srt"
                | "text"
                | "tsv"
                | "txt"
                | "vtt"
                | "wav"
                | "webm"
                | "xlsx"
                | "xml"
        ),
    }
}

fn provider_matches(name: &str, text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    matches!(
        (name, text.as_str()),
        ("provider_bland", "bland") | ("provider_twilio", "twilio") | ("provider_vapi", "vapi")
    )
}

fn api_json(value: &Value) -> bool {
    value.is_object()
        || value.is_array()
        || value.as_str().is_some_and(|text| {
            serde_json::from_str::<Value>(text)
                .ok()
                .is_some_and(|parsed| parsed.is_object() || parsed.is_array())
        })
}

fn parsed_paper_value(value: &Value) -> bool {
    if let Some(items) = value.as_array() {
        return !items.is_empty() && items.iter().all(parsed_paper_value);
    }
    let Some(object) = value.as_object() else {
        return false;
    };
    let has_title = object
        .get("title")
        .and_then(Value::as_str)
        .is_some_and(|title| !title.trim().is_empty());
    let has_valid_id = ["arxiv_id", "id", "eprint"]
        .into_iter()
        .filter_map(|key| object.get(key).and_then(Value::as_str))
        .any(valid_arxiv_id);
    has_title && has_valid_id
}

fn graphql_success(value: &Value) -> bool {
    let parsed;
    let value = if let Some(text) = value.as_str() {
        let Ok(decoded) = serde_json::from_str::<Value>(text.trim()) else {
            return false;
        };
        parsed = decoded;
        &parsed
    } else {
        value
    };
    if !(value.is_object() || value.is_array()) {
        return false;
    }
    graph_response_has_no_errors(value)
}

fn graph_response_has_no_errors(value: &Value) -> bool {
    match value {
        Value::Object(object) => {
            for (key, value) in object {
                if matches!(key.as_str(), "errors" | "userErrors") && !empty_error_value(value) {
                    return false;
                }
                if !graph_response_has_no_errors(value) {
                    return false;
                }
            }
            true
        }
        Value::Array(items) => items.iter().all(graph_response_has_no_errors),
        _ => true,
    }
}

fn empty_error_value(value: &Value) -> bool {
    match value {
        Value::Null => true,
        Value::Array(items) => items.is_empty(),
        Value::Object(object) => object.is_empty(),
        Value::String(text) => text.trim().is_empty(),
        Value::Bool(value) => !value,
        Value::Number(_) => false,
    }
}

fn path_like(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && !has_control(text)
}

fn extension_in(text: &str, extensions: &[&str]) -> bool {
    let lower = text.trim().to_ascii_lowercase();
    extensions
        .iter()
        .any(|extension| lower.ends_with(&format!(".{extension}")))
}

fn positive_text(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && !has_control(text)
}

fn identifier_like(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty() && text.len() <= 512 && !text.chars().any(|ch| ch.is_control())
}

fn json_field_path(text: &str) -> bool {
    let text = text.trim();
    !text.is_empty()
        && text.len() <= 512
        && text
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-' | '.' | '[' | ']'))
}

fn iso8601_with_tz(text: &str) -> bool {
    let text = text.trim();
    text.contains('T')
        && (text.ends_with('Z')
            || text.rsplit_once(['+', '-']).is_some_and(|(_, offset)| {
                offset.len() == 5
                    && offset.as_bytes().get(2) == Some(&b':')
                    && offset
                        .chars()
                        .enumerate()
                        .all(|(index, ch)| index == 2 || ch.is_ascii_digit())
            }))
}

fn json_number(value: &Value) -> Option<f64> {
    value.as_f64().or_else(|| {
        value
            .as_str()
            .and_then(|text| text.trim().parse::<f64>().ok())
    })
}

fn json_integer(value: &Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|number| i64::try_from(number).ok()))
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| text.trim().parse::<i64>().ok())
        })
}

fn has_control(text: &str) -> bool {
    text.chars().any(char::is_control)
}

fn is_identifier(raw: &str) -> bool {
    let mut chars = raw.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    (first == '_' || first.is_ascii_alphabetic())
        && chars.all(|ch| ch == '_' || ch.is_ascii_alphanumeric())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn recognizes_declared_semantic_predicates() {
        assert!(is_supported_expr("is_repo(r)"));
        assert!(is_supported_expr("iso8601_with_tz(t)"));
        assert!(is_supported_expr("gql_success(r)"));
        assert!(is_supported_expr("tool_names_normalized(n)"));
        assert!(!is_supported_expr("plan_approved(p)"));
    }

    #[test]
    fn checks_common_semantic_predicates() {
        assert_eq!(matches(&json!("openai/puffer"), "is_repo(r)"), Some(true));
        assert_eq!(
            matches(&json!("https://youtu.be/abc"), "is_youtube(u)"),
            Some(true)
        );
        assert_eq!(
            matches(&json!("ftp://example.com"), "is_url(u)"),
            Some(false)
        );
        assert_eq!(
            matches(
                &json!("spotify:track:0DiWol3AO6WpXZgp0goxAV"),
                "valid_uri(u)"
            ),
            Some(true)
        );
        assert_eq!(
            matches(
                &json!(r#"["spotify:track:0DiWol3AO6WpXZgp0goxAV"]"#),
                "valid_uri(u)"
            ),
            Some(true)
        );
        assert_eq!(
            matches(&json!("spotify:track:0DiWol3AO6WpXZgp0goxAV"), "is_url(u)"),
            Some(false)
        );
        assert_eq!(matches(&json!("VS Code"), "known_app(a)"), Some(true));
        assert_eq!(matches(&json!("claude"), "valid_provider(p)"), Some(true));
        assert_eq!(matches(&json!("bf16"), "valid_precision(m)"), Some(true));
        assert_eq!(matches(&json!("FSDP"), "valid_backend(b)"), Some(true));
        assert_eq!(matches(&json!("h264"), "codec_license_ok(c)"), Some(false));
        assert_eq!(
            matches(
                &json!(".hermes/plans/2026-05-25_120000-work.md"),
                "plan_path(p)"
            ),
            Some(true)
        );
        assert_eq!(matches(&json!(91.0), "valid_lat(lat)"), Some(false));
        assert_eq!(matches(&json!(1500), "ephemeral_port(p)"), Some(true));
        assert_eq!(
            matches(
                &json!("mcp__agentmail__send_message"),
                "tool_names_normalized(n)"
            ),
            Some(true)
        );
        assert_eq!(
            matches(
                &json!("mcp_agentmail_send_message"),
                "tool_names_normalized(n)"
            ),
            Some(true)
        );
        assert_eq!(
            matches(&json!("agentmail.send_message"), "tool_names_normalized(n)"),
            Some(false)
        );
        assert_eq!(
            matches(
                &json!({"data": {"fulfillmentCreate": {"fulfillment": {"id": "1"}, "userErrors": []}}}),
                "gql_success(r)"
            ),
            Some(true)
        );
        assert_eq!(
            matches(
                &json!({"data": {"fulfillmentCreate": {"userErrors": [{"message": "bad"}]}}}),
                "gql_success(r)"
            ),
            Some(false)
        );
        assert_eq!(
            matches(&json!({"errors": [{"message": "bad"}]}), "gql_success(r)"),
            Some(false)
        );
    }
}
