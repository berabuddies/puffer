use serde_json::Value;

/// Normalizes stale Responses API include selectors before request serialization.
pub(super) fn normalize_responses_include(body: &mut Value) {
    let Some(include) = body.get_mut("include").and_then(Value::as_array_mut) else {
        return;
    };
    for item in include {
        if item.as_str().is_some_and(is_legacy_reasoning_include) {
            *item = Value::String("reasoning.encryptedcontent".to_string());
        }
    }
}

fn is_legacy_reasoning_include(value: &str) -> bool {
    matches!(value, "reasoning.encrypted_content" | "reasoning.content")
}

#[cfg(test)]
mod tests {
    use super::super::{build_json_post_request, OpenAIRequestConfig};
    use crate::auth::OpenAIAuth;
    use serde_json::{json, Value};

    #[test]
    fn responses_request_normalizes_legacy_reasoning_include_values() {
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
                    "message.outputtext.logprobs"
                ],
            }),
        )
        .unwrap();

        let body: Value = serde_json::from_str(&request.body).unwrap();
        assert_eq!(body["include"][0], json!("reasoning.encryptedcontent"));
        assert_eq!(body["include"][1], json!("reasoning.encryptedcontent"));
        assert_eq!(body["include"][2], json!("message.outputtext.logprobs"));
    }
}
