use anyhow::{anyhow, bail, Context, Result};
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
struct WebSearchInput {
    query: String,
    #[serde(default)]
    allowed_domains: Vec<String>,
    #[serde(default)]
    blocked_domains: Vec<String>,
    #[serde(default)]
    user_location: Option<Value>,
    #[serde(default)]
    external_web_access: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SourceLink {
    title: String,
    url: String,
}

pub(super) fn execute_openai_web_search(
    request_config: &OpenAIRequestConfig,
    model_id: &str,
    input: Value,
) -> Result<String> {
    let input: WebSearchInput = serde_json::from_value(input).context("invalid WebSearch input")?;
    validate_input(&input)?;
    if !input.blocked_domains.is_empty() {
        bail!("blocked_domains is not supported by OpenAI web search");
    }

    let request = build_tool_responses_request(
        request_config,
        &OpenAIResponsesToolRequest {
            model: model_id.to_string(),
            input: Value::String(input.query.clone()),
            tools: vec![OpenAIResponsesTool {
                kind: "web_search".to_string(),
                name: String::new(),
                description: String::new(),
                parameters: Value::Null,
                filters: (!input.allowed_domains.is_empty())
                    .then(|| json!({ "allowed_domains": input.allowed_domains })),
                user_location: input.user_location.clone(),
                external_web_access: input.external_web_access,
            }],
            include: vec!["web_search_call.action.sources".to_string()],
            tool_choice: Some(OpenAIResponsesToolChoice::Mode(
                OpenAIResponsesToolChoiceMode::Auto,
            )),
            previous_response_id: None,
        },
    )?;

    let response = send_json_request(&request.url, &request.headers, &request.body, false)?;
    let parsed = parse_responses_response(&serde_json::to_string(&response)?)?;
    let text = extract_responses_text(&parsed);
    if text.trim().is_empty() {
        bail!("OpenAI web search returned no text");
    }
    let sources = extract_openai_sources(&response);
    Ok(format_search_output(text, sources))
}

pub(super) fn execute_anthropic_web_search(
    request_config: &AnthropicRequestConfig,
    model_id: &str,
    input: Value,
) -> Result<String> {
    let input: WebSearchInput = serde_json::from_value(input).context("invalid WebSearch input")?;
    validate_input(&input)?;
    if input.user_location.is_some() {
        bail!("user_location is not supported by Anthropic web search");
    }
    if input.external_web_access.is_some() {
        bail!("external_web_access is not supported by Anthropic web search");
    }

    let request = build_messages_request(
        request_config,
        &AnthropicModelRequest {
            model: model_id.to_string(),
            max_tokens: 1024,
            messages: vec![AnthropicMessage {
                role: "user".to_string(),
                content: format!(
                    "Perform a web search for the following query and provide a concise answer with sources: {}",
                    input.query
                ),
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

    let response = send_json_request(&request.url, &request.headers, &body.to_string(), true)?;
    let text = extract_anthropic_text(&response)?;
    let sources = extract_anthropic_sources(&response);
    Ok(format_search_output(text, sources))
}

fn validate_input(input: &WebSearchInput) -> Result<()> {
    if input.query.trim().len() < 2 {
        bail!("WebSearch query must be at least 2 characters");
    }
    if !input.allowed_domains.is_empty() && !input.blocked_domains.is_empty() {
        bail!("cannot specify both allowed_domains and blocked_domains");
    }
    Ok(())
}

fn send_json_request(
    url: &str,
    headers: &[(String, String)],
    body: &str,
    anthropic: bool,
) -> Result<Value> {
    let client = Client::new();
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
    let response = request
        .body(body.to_string())
        .send()
        .with_context(|| format!("request to {url} failed"))?;
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
