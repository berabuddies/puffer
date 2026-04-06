use crate::OAUTH_BETA_HEADER;
use anyhow::{Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;

/// One Anthropic OAuth usage limit bucket.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AnthropicRateLimit {
    pub utilization: Option<f64>,
    pub resets_at: Option<String>,
}

/// Anthropic extra-usage metadata for subscriber billing.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AnthropicExtraUsage {
    pub is_enabled: bool,
    pub monthly_limit: Option<f64>,
    pub used_credits: Option<f64>,
    pub utilization: Option<f64>,
}

/// Anthropic OAuth subscriber utilization response.
#[derive(Debug, Clone, Deserialize, PartialEq)]
pub struct AnthropicUtilization {
    pub five_hour: Option<AnthropicRateLimit>,
    pub seven_day: Option<AnthropicRateLimit>,
    pub seven_day_oauth_apps: Option<AnthropicRateLimit>,
    pub seven_day_opus: Option<AnthropicRateLimit>,
    pub seven_day_sonnet: Option<AnthropicRateLimit>,
    pub extra_usage: Option<AnthropicExtraUsage>,
}

/// Fetches Anthropic OAuth usage data using the subscriber usage endpoint.
pub fn fetch_oauth_usage(base_url: &str, access_token: &str) -> Result<AnthropicUtilization> {
    let client = Client::new();
    let response = client
        .get(format!(
            "{}/api/oauth/usage",
            base_url.trim_end_matches('/')
        ))
        .header("Authorization", format!("Bearer {access_token}"))
        .header("anthropic-beta", OAUTH_BETA_HEADER)
        .header(
            "User-Agent",
            format!("claude-code/{}", env!("CARGO_PKG_VERSION")),
        )
        .header("Content-Type", "application/json")
        .send()
        .context("failed to fetch Anthropic OAuth usage")?;
    let status = response.status();
    let usage: AnthropicUtilization = response
        .json()
        .context("failed to parse Anthropic OAuth usage response")?;
    if !status.is_success() {
        anyhow::bail!("Anthropic OAuth usage request failed with status {status}");
    }
    Ok(usage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn fetch_oauth_usage_uses_expected_headers() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || -> String {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let size = stream.read(&mut buffer).expect("request");
            let request = String::from_utf8_lossy(&buffer[..size]).to_string();
            let body = serde_json::json!({
                "five_hour": { "utilization": 42.0, "resets_at": "2026-04-06T12:00:00Z" }
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream
                .write_all(response.as_bytes())
                .expect("response write");
            request
        });

        let usage = fetch_oauth_usage(&format!("http://{address}"), "subscriber-token")
            .expect("usage fetch");
        let request = server.join().expect("join");
        assert_eq!(usage.five_hour.expect("bucket").utilization, Some(42.0));
        assert!(request.contains("GET /api/oauth/usage HTTP/1.1"));
        let request = request.to_ascii_lowercase();
        assert!(request.contains("authorization: bearer subscriber-token"));
        assert!(request.contains("anthropic-beta: oauth-2025-04-20"));
        assert!(request.contains("user-agent: claude-code/"));
    }
}
