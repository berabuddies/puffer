use serde_json::Value;
use std::collections::HashSet;

/// Normalizes stale Responses API include selectors before request serialization.
pub(super) fn normalize_responses_include(body: &mut Value) {
    let Some(include) = body.get_mut("include").and_then(Value::as_array_mut) else {
        return;
    };
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for item in std::mem::take(include) {
        let Some(selector) = item.as_str().and_then(normalize_include_selector) else {
            continue;
        };
        if seen.insert(selector) {
            normalized.push(Value::String(selector.to_string()));
        }
    }
    *include = normalized;
}

fn normalize_include_selector(value: &str) -> Option<&'static str> {
    match value {
        "filesearchcall.results" | "file_search_call.results" => Some("filesearchcall.results"),
        "websearchcall.results" | "web_search_call.results" => Some("websearchcall.results"),
        "websearchcall.action.sources" | "web_search_call.action.sources" => {
            Some("websearchcall.action.sources")
        }
        "message.inputimage.imageurl" | "message.input_image.image_url" => {
            Some("message.inputimage.imageurl")
        }
        "computercalloutput.output.imageurl" | "computer_call_output.output.image_url" => {
            Some("computercalloutput.output.imageurl")
        }
        "codeinterpretercall.outputs" | "code_interpreter_call.outputs" => {
            Some("codeinterpretercall.outputs")
        }
        "reasoning.encryptedcontent" | "reasoning.encrypted_content" | "reasoning.content" => {
            Some("reasoning.encryptedcontent")
        }
        "message.outputtext.logprobs" | "message.output_text.logprobs" => {
            Some("message.outputtext.logprobs")
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::super::{build_json_post_request, OpenAIRequestConfig};
    use crate::auth::OpenAIAuth;
    use serde_json::{json, Value};

    #[test]
    fn responses_request_sanitizes_and_deduplicates_include_values() {
        let request = build_json_post_request(
            &OpenAIRequestConfig {
                base_url: "https://api.openai.com".to_string(),
                version: "0.1.0".to_string(),
                auth: OpenAIAuth::ApiKey("sk-test".to_string()),
                originator: "codex_cli_rs".to_string(),
                session_id: None,
                account_id: None,
                custom_headers: Vec::new(),
                query_params: Vec::new(),
                chat_completions_path: None,
                responses_path: None,
            },
            "/v1/responses",
            &json!({
                "model": "gpt-5",
                "include": [
                    "reasoning.encrypted_content",
                    "reasoning.content",
                    "message.output_text.logprobs",
                    "web_search_call.action.sources",
                    "unknown.future.selector",
                    7
                ],
            }),
        )
        .unwrap();

        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["include"][0], json!("reasoning.encryptedcontent"));
        assert_eq!(body["include"][1], json!("message.outputtext.logprobs"));
        assert_eq!(body["include"][2], json!("websearchcall.action.sources"));
        assert_eq!(body["include"].as_array().unwrap().len(), 3);
    }
}
