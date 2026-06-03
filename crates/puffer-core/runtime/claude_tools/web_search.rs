use super::retry_http_send;
use crate::runtime::openai_sse::openai_response_incomplete_error;
use anyhow::{anyhow, bail, Context, Result};
use puffer_config::ProxyConfig;
use puffer_provider_openai::{
    build_tool_responses_request, extract_responses_text, parse_responses_response,
    OpenAIRequestConfig, OpenAIResponsesTool, OpenAIResponsesToolChoice,
    OpenAIResponsesToolChoiceMode, OpenAIResponsesToolRequest,
};
use puffer_transport_anthropic::{
    build_messages_request, AnthropicMessage, AnthropicModelRequest, AnthropicRequestConfig,
};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;

#[derive(Debug, Deserialize)]
struct ClaudeWebSearchInput {
    query: String,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    blocked_domains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceLink {
    title: String,
    url: String,
}

fn claude_web_search_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "query": {
                "type": "string",
                "description": "The search query to use",
            },
            "allowed_domains": {
                "type": "array",
                "description": "Only include search results from these domains",
            },
            "blocked_domains": {
                "type": "array",
                "description": "Never include search results from these domains",
            },
        },
        "required": ["query"],
        "additionalProperties": false,
    })
}

/// Executes Claude-style WebSearch against an OpenAI Responses-compatible endpoint.
pub fn execute_claude_openai_web_search(
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    raw_input: Value,
    proxy: &ProxyConfig,
) -> Result<String> {
    let input = parse_input(raw_input)?;

    let request = build_tool_responses_request(
        request_config,
        &OpenAIResponsesToolRequest {
            model: model_id.to_string(),
            input: Value::String(input.query.clone()),
            tools: vec![OpenAIResponsesTool {
                kind: "web_search".to_string(),
                name: String::new(),
                description: String::new(),
                strict: false,
                parameters: Value::Null,
                filters: build_openai_filters(&input),
                user_location: None,
                external_web_access: None,
            }],
            include: Vec::new(),
            tool_choice: Some(OpenAIResponsesToolChoice::Mode(
                OpenAIResponsesToolChoiceMode::Auto,
            )),
            previous_response_id: None,
            text: None,
        },
    )?;

    let client = crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::Model,
        &request.url,
        std::time::Duration::from_secs(300),
    )
    .unwrap_or_else(|_| Client::new());
    let response = send_json_request_with_client(
        &client,
        &request.url,
        &request.headers,
        &request.body,
        false,
    )?;
    let (text, sources) = parse_openai_web_search_response(&response)?;

    Ok(format_search_output(text, sources))
}

fn parse_openai_web_search_response(response: &Value) -> Result<(String, Vec<SourceLink>)> {
    if let Some(error) = openai_response_incomplete_error(response) {
        return Err(error);
    }
    let parsed = parse_responses_response(&serde_json::to_string(response)?)?;
    let text = extract_responses_text(&parsed);
    if text.trim().is_empty() {
        bail!("OpenAI web search returned no text");
    }

    Ok((text, extract_openai_sources(response)))
}

/// Executes Claude-style WebSearch against an Anthropic Messages-compatible endpoint.
pub fn execute_claude_anthropic_web_search(
    request_config: &AnthropicRequestConfig,
    model_id: &str,
    raw_input: Value,
    proxy: &ProxyConfig,
) -> Result<String> {
    let input = parse_input(raw_input)?;

    let request = build_messages_request(
        request_config,
        &AnthropicModelRequest {
            model: model_id.to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: format!("Perform a web search for the query: {}", input.query),
            }],
        },
    )?;
    let mut body: Value =
        serde_json::from_str(&request.body).context("failed to parse Anthropic request body")?;
    body["tools"] = Value::Array(vec![json!({
        "type": "web_search_20250305",
        "name": "web_search",
        "allowed_domains": if input.allowed_domains.is_empty() {
            Value::Null
        } else {
            json!(input.allowed_domains)
        },
        "blocked_domains": if input.blocked_domains.is_empty() {
            Value::Null
        } else {
            json!(input.blocked_domains)
        },
        "max_uses": 8,
    })]);
    body["tool_choice"] = json!({ "type": "auto" });

    let client = crate::network::blocking_client_for_url(
        proxy,
        crate::network::HttpPurpose::Model,
        &request.url,
        std::time::Duration::from_secs(300),
    )
    .unwrap_or_else(|_| Client::new());
    let response = send_json_request_with_client(
        &client,
        &request.url,
        &request.headers,
        &body.to_string(),
        true,
    )?;
    let text = extract_anthropic_text(&response)?;
    Ok(format_search_output(
        text,
        extract_anthropic_sources(&response),
    ))
}

fn parse_input(raw_input: Value) -> Result<ClaudeWebSearchInput> {
    let input: ClaudeWebSearchInput =
        serde_json::from_value(raw_input).context("invalid WebSearch input")?;
    if input.query.trim().len() < 2 {
        bail!("WebSearch query must be at least 2 characters");
    }
    if !input.allowed_domains.is_empty() && !input.blocked_domains.is_empty() {
        bail!("cannot specify both allowed_domains and blocked_domains in one request");
    }
    Ok(input)
}

fn build_openai_filters(input: &ClaudeWebSearchInput) -> Option<Value> {
    let mut filters = serde_json::Map::new();
    if !input.allowed_domains.is_empty() {
        filters.insert("allowed_domains".to_string(), json!(input.allowed_domains));
    }
    if !input.blocked_domains.is_empty() {
        filters.insert("blocked_domains".to_string(), json!(input.blocked_domains));
    }
    (!filters.is_empty()).then(|| Value::Object(filters))
}

fn send_json_request_with_client(
    client: &Client,
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<Value> {
    let response = retry_http_send(3, || {
        let mut request = client.post(url);
        for (key, value) in headers {
            request = request.header(key, value);
        }
        if !headers
            .iter()
            .any(|(key, _)| key.eq_ignore_ascii_case("content-type"))
        {
            request = request.header("content-type", "application/json");
        }
        if anthropic
            && !headers
                .iter()
                .any(|(key, _)| key.eq_ignore_ascii_case("anthropic-version"))
        {
            request = request.header("anthropic-version", "2023-06-01");
        }
        request
            .body(body.to_string())
            .send()
            .with_context(|| format!("request to {url} failed"))
    })?;
    let status = response.status();
    let text = response.text()?;
    if !status.is_success() {
        bail!("request failed with status {}: {}", status, text);
    }
    serde_json::from_str::<Value>(&text)
        .with_context(|| format!("response from {url} was not valid JSON"))
}

fn extract_openai_sources(response: &Value) -> Vec<SourceLink> {
    let mut sources = BTreeSet::new();
    if let Some(output) = response.get("output").and_then(Value::as_array) {
        for item in output {
            if item.get("type").and_then(Value::as_str) == Some("web_search_call") {
                if let Some(entries) = item
                    .get("action")
                    .and_then(|action| action.get("sources"))
                    .and_then(Value::as_array)
                {
                    for source in entries {
                        insert_source(
                            &mut sources,
                            source.get("title").and_then(Value::as_str),
                            source.get("url").and_then(Value::as_str),
                        );
                    }
                }
            }
            if item.get("type").and_then(Value::as_str) == Some("message") {
                if let Some(content) = item.get("content").and_then(Value::as_array) {
                    for block in content {
                        if let Some(annotations) =
                            block.get("annotations").and_then(Value::as_array)
                        {
                            for annotation in annotations {
                                insert_source(
                                    &mut sources,
                                    annotation.get("title").and_then(Value::as_str),
                                    annotation.get("url").and_then(Value::as_str),
                                );
                            }
                        }
                    }
                }
            }
        }
    }
    sources.into_iter().collect()
}

fn extract_anthropic_sources(response: &Value) -> Vec<SourceLink> {
    let mut sources = BTreeSet::new();
    if let Some(content) = response.get("content").and_then(Value::as_array) {
        for block in content {
            if block.get("type").and_then(Value::as_str) == Some("web_search_tool_result") {
                if let Some(entries) = block.get("content").and_then(Value::as_array) {
                    for source in entries {
                        insert_source(
                            &mut sources,
                            source.get("title").and_then(Value::as_str),
                            source.get("url").and_then(Value::as_str),
                        );
                    }
                }
            }
        }
    }
    sources.into_iter().collect()
}

fn extract_anthropic_text(response: &Value) -> Result<String> {
    let parts = response
        .get("content")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("anthropic response missing content array"))?
        .iter()
        .filter_map(|item| {
            let item_type = item.get("type").and_then(Value::as_str)?;
            if item_type == "text" {
                item.get("text").and_then(Value::as_str).map(str::to_string)
            } else {
                None
            }
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        bail!("anthropic response did not contain text content");
    }
    Ok(parts.join("\n"))
}

fn insert_source(sources: &mut BTreeSet<SourceLink>, title: Option<&str>, url: Option<&str>) {
    let Some(url) = url.filter(|value| !value.trim().is_empty()) else {
        return;
    };
    let title = title
        .filter(|value| !value.trim().is_empty())
        .unwrap_or(url)
        .to_string();
    sources.insert(SourceLink {
        title,
        url: url.to_string(),
    });
}

fn format_search_output(text: String, sources: Vec<SourceLink>) -> String {
    if sources.is_empty() {
        return text.trim().to_string();
    }
    let mut output = text.trim().to_string();
    output.push_str("\n\nSources:\n");
    for source in sources {
        output.push_str(&format!("- [{}]({})\n", source.title, source.url));
    }
    output.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_matches_claude_field_names() {
        let schema = claude_web_search_input_schema();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap();
        assert!(properties.contains_key("query"));
        assert!(properties.contains_key("allowed_domains"));
        assert!(properties.contains_key("blocked_domains"));
        assert!(!properties.contains_key("user_location"));
        assert!(!properties.contains_key("external_web_access"));
    }

    #[test]
    fn parse_input_rejects_conflicting_domain_filters() {
        let error = parse_input(json!({
            "query": "rust",
            "allowed_domains": ["example.com"],
            "blocked_domains": ["another.example"],
        }))
        .unwrap_err()
        .to_string();
        assert!(error.contains("cannot specify both"));
    }

    #[test]
    fn build_openai_filters_includes_blocked_domains() {
        let filters = build_openai_filters(&ClaudeWebSearchInput {
            query: "rust".to_string(),
            allowed_domains: Vec::new(),
            blocked_domains: vec!["example.com".to_string()],
        })
        .unwrap();
        assert_eq!(filters["blocked_domains"], json!(["example.com"]));
    }

    #[test]
    fn format_search_output_appends_sources_section() {
        let output = format_search_output(
            "Result body".to_string(),
            vec![
                SourceLink {
                    title: "One".to_string(),
                    url: "https://one.example".to_string(),
                },
                SourceLink {
                    title: "Two".to_string(),
                    url: "https://two.example".to_string(),
                },
            ],
        );
        assert!(output.contains("Sources:"));
        assert!(output.contains("- [One](https://one.example)"));
        assert!(output.contains("- [Two](https://two.example)"));
    }

    #[test]
    fn openai_web_search_rejects_incomplete_response() {
        let error = parse_openai_web_search_response(&json!({
            "id": "resp_incomplete",
            "status": "incomplete",
            "incomplete_details": {"reason": "content_filter"},
            "output": [{
                "type": "message",
                "content": [{"type": "output_text", "text": "partial"}]
            }]
        }))
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "Incomplete response returned, reason: content_filter"
        );
    }
}
