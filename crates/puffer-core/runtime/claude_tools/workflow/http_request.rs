use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::path::Path;
use std::time::Duration;

const DEFAULT_TIMEOUT_MS: u64 = 60_000;
const MAX_TIMEOUT_MS: u64 = 300_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct HttpRequestInput {
    method: String,
    url: String,
    #[serde(default)]
    headers: BTreeMap<String, String>,
    #[serde(default)]
    json: Option<Value>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    timeout_ms: Option<u64>,
    #[serde(default)]
    fail_on_http_error: Option<bool>,
}

/// Executes a fully declared HTTP request for verified Lambda skill contracts.
pub fn execute_http_request(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: HttpRequestInput =
        serde_json::from_value(input).context("invalid HttpRequest input")?;
    let method = parse_method(&parsed.method)?;
    let url = parse_url(&parsed.url)?;
    let headers = parse_headers(&parsed.headers)?;
    if parsed.json.is_some() && parsed.body.is_some() {
        bail!("HttpRequest accepts either json or body, not both");
    }

    let timeout = parsed
        .timeout_ms
        .unwrap_or(DEFAULT_TIMEOUT_MS)
        .clamp(1, MAX_TIMEOUT_MS);
    let client = Client::builder()
        .timeout(Duration::from_millis(timeout))
        .build()
        .context("build HTTP client")?;
    let mut request = client.request(method, url).headers(headers);
    if let Some(json_body) = parsed.json {
        request = request.json(&json_body);
    } else if let Some(body) = parsed.body {
        request = request.body(body);
    }

    let response = request.send().context("send HTTP request")?;
    let status = response.status();
    let response_headers = response
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_ascii_lowercase(),
                value.to_str().unwrap_or("<binary>").to_string(),
            )
        })
        .collect::<BTreeMap<_, _>>();
    let body = response.text().context("read HTTP response body")?;
    if parsed.fail_on_http_error.unwrap_or(true) && !status.is_success() {
        bail!(
            "HTTP request failed with status {}: {}",
            status.as_u16(),
            body
        );
    }

    Ok(serde_json::to_string_pretty(&json!({
        "ok": status.is_success(),
        "status": status.as_u16(),
        "statusText": status.canonical_reason().unwrap_or(""),
        "headers": response_headers,
        "body": body,
    }))?)
}

fn parse_method(method: &str) -> Result<Method> {
    match method.trim().to_ascii_uppercase().as_str() {
        "GET" => Ok(Method::GET),
        "POST" => Ok(Method::POST),
        "PUT" => Ok(Method::PUT),
        "PATCH" => Ok(Method::PATCH),
        "DELETE" => Ok(Method::DELETE),
        other => bail!("unsupported HTTP method `{other}`"),
    }
}

fn parse_url(url: &str) -> Result<reqwest::Url> {
    let parsed = reqwest::Url::parse(url).context("HttpRequest url must be absolute")?;
    match parsed.scheme() {
        "http" | "https" => Ok(parsed),
        other => bail!("unsupported URL scheme `{other}`; use http or https"),
    }
}

fn parse_headers(headers: &BTreeMap<String, String>) -> Result<HeaderMap> {
    let mut map = HeaderMap::new();
    for (name, value) in headers {
        let header_name = HeaderName::from_bytes(name.as_bytes())
            .with_context(|| format!("invalid HTTP header name `{name}`"))?;
        let header_value = HeaderValue::from_str(value)
            .with_context(|| format!("invalid value for HTTP header `{name}`"))?;
        map.insert(header_name, header_value);
    }
    Ok(map)
}
