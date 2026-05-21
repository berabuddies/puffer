use super::retry_http_send;
use anyhow::{bail, Context, Result};
use html2text::from_read;
use reqwest::blocking::{Client, Response};
use reqwest::redirect::Policy;
use reqwest::{StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs;
use std::io::Read;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const CLAUDE_WEB_FETCH_TOOL_DESCRIPTION: &str = r#"- Fetches content from a specified URL and processes it using an AI model
- Takes a URL and a prompt as input
- Fetches the URL content, converts HTML to markdown
- Processes the content with a small, fast model
- Returns the model's response about the content
- Use this tool when you need to retrieve and analyze web content

Usage notes:
  - IMPORTANT: If an MCP-provided web fetch tool is available, prefer using that tool because it may have fewer restrictions.
  - The URL must be a fully-formed valid URL
  - HTTP URLs are automatically upgraded to HTTPS
  - The prompt should describe what information to extract from the page
  - This tool is read-only and does not modify files
  - Results may be summarized if the content is very large
  - Includes a self-cleaning 15-minute cache for repeated fetches
  - If a URL redirects to a different host, run WebFetch again with the redirect URL.
  - For GitHub URLs, prefer using `gh` via Bash (for example `gh pr view`, `gh issue view`, `gh api`)."#;

const MAX_RESULT_CHARS: usize = 100_000;
const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const CACHE_TTL: Duration = Duration::from_secs(15 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);

#[derive(Debug, Clone)]
struct CachedFetch {
    body: String,
    bytes: usize,
    code: u16,
    code_text: String,
    final_url: String,
    inserted_at: Instant,
}

fn web_fetch_cache() -> &'static Mutex<HashMap<String, CachedFetch>> {
    static CACHE: OnceLock<Mutex<HashMap<String, CachedFetch>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

#[derive(Debug, Deserialize)]
struct ClaudeWebFetchInput {
    url: String,
    prompt: String,
}

/// Returns the Claude-compatible WebFetch tool description text.
pub fn claude_web_fetch_tool_description() -> &'static str {
    CLAUDE_WEB_FETCH_TOOL_DESCRIPTION
}

/// Returns the Claude-compatible WebFetch input JSON schema.
pub fn claude_web_fetch_input_schema() -> Value {
    json!({
        "type": "object",
        "properties": {
            "url": {
                "type": "string",
                "description": "The URL to fetch content from",
            },
            "prompt": {
                "type": "string",
                "description": "The prompt to run on the fetched content",
            },
        },
        "required": ["url", "prompt"],
        "additionalProperties": false,
    })
}

/// Executes a Claude-style WebFetch request and returns Claude-compatible output fields.
pub fn execute_claude_web_fetch(raw_input: Value) -> Result<Value> {
    let input: ClaudeWebFetchInput =
        serde_json::from_value(raw_input).context("invalid WebFetch input")?;
    let client = web_fetch_http_client()?;
    execute_claude_web_fetch_internal(client, input)
}

fn web_fetch_http_client() -> Result<Client> {
    Client::builder()
        .timeout(REQUEST_TIMEOUT)
        .redirect(Policy::none())
        .build()
        .context("failed to build WebFetch HTTP client")
}

fn execute_claude_web_fetch_internal(client: Client, input: ClaudeWebFetchInput) -> Result<Value> {
    let start = Instant::now();
    let request_url = normalize_url(&input.url)?;

    if let Some(cached) = cached_fetch(&request_url) {
        let result = apply_prompt_to_content(&input.prompt, &cached.body);
        return Ok(json!({
            "bytes": cached.bytes,
            "code": cached.code,
            "codeText": cached.code_text,
            "result": result?,
            "durationMs": start.elapsed().as_millis(),
            "url": cached.final_url,
        }));
    }

    let response = retry_http_send(3, || {
        client
            .get(request_url.clone())
            .send()
            .with_context(|| format!("failed to fetch {}", request_url))
    })?;
    let status = response.status();
    let final_url = response.url().clone();
    let status_text = status_text(status).to_string();

    if let Some(redirect_url) = redirect_target(&request_url, &response)? {
        let redirect_message =
            build_redirect_message(&request_url, &redirect_url, status, &input.prompt);
        return Ok(json!({
            "bytes": redirect_message.len(),
            "code": status.as_u16(),
            "codeText": status_text,
            "result": redirect_message,
            "durationMs": start.elapsed().as_millis(),
            "url": redirect_url.to_string(),
        }));
    }

    let content_type = response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string);
    let bytes = read_limited_response_body(response)?;
    let content = normalize_web_content(&bytes, content_type.as_deref())?;
    store_cached_fetch(
        &request_url,
        CachedFetch {
            body: content.clone(),
            bytes: bytes.len(),
            code: status.as_u16(),
            code_text: status_text.clone(),
            final_url: final_url.to_string(),
            inserted_at: Instant::now(),
        },
    );
    let result = apply_prompt_to_content(&input.prompt, &content)?;
    Ok(json!({
        "bytes": bytes.len(),
        "code": status.as_u16(),
        "codeText": status_text,
        "result": result,
        "durationMs": start.elapsed().as_millis(),
        "url": final_url.to_string(),
    }))
}

fn normalize_url(raw_url: &str) -> Result<Url> {
    let mut url = Url::parse(raw_url).with_context(|| format!("invalid URL `{raw_url}`"))?;
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|_| anyhow::anyhow!("failed to upgrade URL scheme to https"))?;
    }
    validate_fetch_target(&url)?;
    Ok(url)
}

fn validate_fetch_target(url: &Url) -> Result<()> {
    let host = url
        .host_str()
        .ok_or_else(|| anyhow::anyhow!("WebFetch URL must include a host"))?;
    let lower = host.to_ascii_lowercase();
    if lower == "localhost" || lower.ends_with(".localhost") {
        bail!("WebFetch cannot access localhost targets");
    }
    if let Ok(ip) = host.parse::<IpAddr>() {
        if ip_is_private_or_local(ip) {
            bail!("WebFetch cannot access private or local network targets");
        }
    }
    Ok(())
}

fn ip_is_private_or_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            ip.is_private()
                || ip.is_loopback()
                || ip.is_link_local()
                || ip.is_broadcast()
                || ip.is_documentation()
                || ip == Ipv4Addr::UNSPECIFIED
        }
        IpAddr::V6(ip) => {
            ip.is_loopback()
                || ip.is_unspecified()
                || ip.is_unique_local()
                || ip.is_unicast_link_local()
                || is_ipv6_documentation(ip)
        }
    }
}

fn is_ipv6_documentation(ip: Ipv6Addr) -> bool {
    (ip.segments()[0] & 0xfff0) == 0x2001 && ip.segments()[1] == 0x0db8
}

fn read_limited_response_body(response: reqwest::blocking::Response) -> Result<Vec<u8>> {
    if content_length_exceeds_limit(response.content_length()) {
        bail!(
            "WebFetch response body is too large (limit {} bytes)",
            MAX_RESPONSE_BYTES
        );
    }
    let mut limited = response.take((MAX_RESPONSE_BYTES + 1) as u64);
    let mut bytes = Vec::new();
    limited
        .read_to_end(&mut bytes)
        .context("failed to read web fetch response body")?;
    if bytes.len() > MAX_RESPONSE_BYTES {
        bail!(
            "WebFetch response body is too large (limit {} bytes)",
            MAX_RESPONSE_BYTES
        );
    }
    Ok(bytes)
}

fn content_length_exceeds_limit(length: Option<u64>) -> bool {
    length.is_some_and(|length| length > MAX_RESPONSE_BYTES as u64)
}

fn redirect_target(request_url: &Url, response: &Response) -> Result<Option<Url>> {
    if !response.status().is_redirection() {
        return Ok(None);
    }
    let Some(location) = response.headers().get(reqwest::header::LOCATION) else {
        return Ok(None);
    };
    let location = location
        .to_str()
        .context("redirect location header is not valid UTF-8")?;
    let redirect_url = request_url
        .join(location)
        .with_context(|| format!("invalid redirect location `{location}`"))?;
    Ok(Some(redirect_url))
}

fn status_text(status: StatusCode) -> &'static str {
    status.canonical_reason().unwrap_or("Unknown")
}

fn build_redirect_message(
    request_url: &Url,
    redirect_url: &Url,
    status: StatusCode,
    prompt: &str,
) -> String {
    format!(
        "REDIRECT DETECTED: The URL redirects to a different host.\n\nOriginal URL: {original}\nRedirect URL: {redirect}\nStatus: {code} {code_text}\n\nTo complete your request, use WebFetch again with these parameters:\n- url: \"{redirect}\"\n- prompt: \"{prompt}\"",
        original = request_url,
        redirect = redirect_url,
        code = status.as_u16(),
        code_text = status_text(status),
        prompt = prompt
    )
}

fn cached_fetch(url: &Url) -> Option<CachedFetch> {
    let mut cache = web_fetch_cache().lock().ok()?;
    let entry = cache.get(url.as_str())?.clone();
    if entry.inserted_at.elapsed() > CACHE_TTL {
        cache.remove(url.as_str());
        return None;
    }
    Some(entry)
}

fn store_cached_fetch(url: &Url, entry: CachedFetch) {
    if let Ok(mut cache) = web_fetch_cache().lock() {
        cache.insert(url.to_string(), entry);
    }
}

fn normalize_web_content(bytes: &[u8], content_type: Option<&str>) -> Result<String> {
    if is_binary_content_type(content_type) {
        return build_binary_notice(bytes, content_type);
    }
    let text = String::from_utf8_lossy(bytes).to_string();
    if !looks_like_html(&text) {
        return Ok(text);
    }
    Ok(html_to_text(&text))
}

fn looks_like_html(text: &str) -> bool {
    let lower = text.trim_start().to_ascii_lowercase();
    lower.starts_with("<!doctype html")
        || lower.starts_with("<html")
        || lower.contains("<body")
        || lower.contains("<p>")
}

fn is_binary_content_type(content_type: Option<&str>) -> bool {
    let Some(content_type) = content_type else {
        return false;
    };
    let content_type = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    !(content_type.starts_with("text/")
        || matches!(
            content_type.as_str(),
            "application/json"
                | "application/ld+json"
                | "application/javascript"
                | "application/x-javascript"
                | "application/xml"
                | "application/xhtml+xml"
                | "application/x-yaml"
                | "application/yaml"
                | "image/svg+xml"
        ))
}

fn build_binary_notice(bytes: &[u8], content_type: Option<&str>) -> Result<String> {
    let path = persist_binary_payload(bytes, content_type)?;
    let mime = content_type.unwrap_or("application/octet-stream");
    Ok(format!(
        "Binary content saved to: {path}\nContent-Type: {mime}\nBytes: {}\n\nWebFetch cannot analyze binary content directly. Use an appropriate local tool on the saved file if you need to inspect it further.",
        bytes.len()
    ))
}

fn persist_binary_payload(bytes: &[u8], content_type: Option<&str>) -> Result<String> {
    let root = std::env::temp_dir().join("puffer-web-fetch");
    fs::create_dir_all(&root).with_context(|| format!("failed to create {}", root.display()))?;
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let extension = extension_for_content_type(content_type);
    let file_name = format!("fetch-{timestamp}{extension}");
    let path = root.join(file_name);
    fs::write(&path, bytes).with_context(|| format!("failed to write {}", path.display()))?;
    Ok(path.display().to_string())
}

fn extension_for_content_type(content_type: Option<&str>) -> &'static str {
    let Some(content_type) = content_type else {
        return ".bin";
    };
    match content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
    {
        "application/pdf" => ".pdf",
        "image/png" => ".png",
        "image/jpeg" => ".jpg",
        "image/gif" => ".gif",
        "image/webp" => ".webp",
        "application/zip" => ".zip",
        "application/json" => ".json",
        _ => ".bin",
    }
}

fn html_to_text(html: &str) -> String {
    let stripped = strip_tag_block(&strip_tag_block(html, "script"), "style");
    from_read(stripped.as_bytes(), 80)
        .unwrap_or_else(|_| stripped)
        .trim()
        .to_string()
}

fn strip_tag_block(html: &str, tag: &str) -> String {
    let lower = html.to_ascii_lowercase();
    let open = format!("<{tag}");
    let close = format!("</{tag}>");
    let mut result = String::new();
    let mut index = 0usize;
    while let Some(relative_start) = lower[index..].find(&open) {
        let start = index + relative_start;
        result.push_str(&html[index..start]);
        let Some(relative_end) = lower[start..].find(&close) else {
            return result;
        };
        let end = start + relative_end + close.len();
        index = end;
    }
    result.push_str(&html[index..]);
    result
}

fn apply_prompt_to_content(prompt: &str, content: &str) -> Result<String> {
    let trimmed_prompt = prompt.trim();
    if trimmed_prompt.is_empty() {
        bail!("WebFetch prompt cannot be empty");
    }
    let mut result = format!(
        "Prompt:\n{prompt}\n\nFetched Content:\n",
        prompt = trimmed_prompt
    );
    if content.chars().count() > MAX_RESULT_CHARS {
        result.push_str(&truncate_for_result(content, MAX_RESULT_CHARS));
        result.push_str("\n\n[Result truncated due to size]");
    } else {
        result.push_str(content);
    }
    Ok(result)
}

fn truncate_for_result(content: &str, max_chars: usize) -> String {
    content.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::Path;

    #[test]
    fn schema_matches_claude_field_names() {
        let schema = claude_web_fetch_input_schema();
        let properties = schema.get("properties").and_then(Value::as_object).unwrap();
        assert!(properties.contains_key("url"));
        assert!(properties.contains_key("prompt"));
    }

    #[test]
    fn normalize_url_upgrades_http() {
        let url = normalize_url("http://example.com/path").unwrap();
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("example.com"));
    }

    #[test]
    fn normalize_url_rejects_localhost_targets() {
        let error = normalize_url("https://localhost/path").unwrap_err();
        assert!(error.to_string().contains("localhost"));
    }

    #[test]
    fn normalize_url_rejects_private_ip_targets() {
        let error = normalize_url("https://192.168.1.10/path").unwrap_err();
        assert!(error.to_string().contains("private or local"));
    }

    #[test]
    fn content_length_limit_rejects_large_responses() {
        assert!(content_length_exceeds_limit(Some(
            (MAX_RESPONSE_BYTES + 1) as u64
        )));
        assert!(!content_length_exceeds_limit(Some(
            MAX_RESPONSE_BYTES as u64
        )));
        assert!(!content_length_exceeds_limit(None));
    }

    #[test]
    fn redirect_message_contains_follow_up_instruction() {
        let original = Url::parse("https://old.example").unwrap();
        let redirect = Url::parse("https://new.example/page").unwrap();
        let message =
            build_redirect_message(&original, &redirect, StatusCode::FOUND, "extract title");
        assert!(message.contains("REDIRECT DETECTED"));
        assert!(message.contains("use WebFetch again"));
        assert!(message.contains("https://new.example/page"));
    }

    #[test]
    fn web_fetch_client_does_not_follow_redirects() {
        use std::io::{Read as _, Write as _};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 1024];
            let _ = stream.read(&mut buffer).unwrap();
            stream
                .write_all(
                    b"HTTP/1.1 302 Found\r\nLocation: http://127.0.0.1:9/metadata\r\nContent-Length: 0\r\n\r\n",
                )
                .unwrap();
        });

        let client = web_fetch_http_client().unwrap();
        let request_url = Url::parse(&format!("http://{addr}/start")).unwrap();
        let response = client.get(request_url.clone()).send().unwrap();

        handle.join().unwrap();
        assert_eq!(response.status(), StatusCode::FOUND);
        let redirect = redirect_target(&request_url, &response).unwrap().unwrap();
        assert_eq!(redirect.as_str(), "http://127.0.0.1:9/metadata");
    }

    #[test]
    fn apply_prompt_to_content_prefixes_prompt() {
        let output = apply_prompt_to_content("summarize", "hello world").unwrap();
        assert!(output.contains("Prompt:\nsummarize"));
        assert!(output.contains("Fetched Content:\nhello world"));
    }

    #[test]
    fn html_content_is_normalized_before_prompting() {
        let normalized = normalize_web_content(
            b"<html><body><h1>Title</h1><p>Hello<br>world</p></body></html>",
            Some("text/html"),
        )
        .unwrap();
        assert!(normalized.contains("Title"));
        assert!(normalized.contains("Hello"));
        assert!(normalized.contains("world"));
        assert!(!normalized.contains("<h1>"));
    }

    #[test]
    fn truncate_for_result_uses_char_boundary() {
        let truncated = truncate_for_result("aé漢字", 3);
        assert_eq!(truncated, "aé漢");
    }

    #[test]
    fn binary_content_creates_saved_file_notice() {
        let notice = normalize_web_content(b"%PDF-1.7", Some("application/pdf")).unwrap();
        assert!(notice.contains("Binary content saved to:"));
        let path = notice
            .lines()
            .next()
            .and_then(|line| line.strip_prefix("Binary content saved to: "))
            .unwrap();
        assert!(Path::new(path).exists());
        let _ = fs::remove_file(path);
    }
}
