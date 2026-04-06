use reqwest::blocking::Client;
use serde::Deserialize;
use thiserror::Error;

const COMPLETIONS_USAGE_PATH: &str = "/v1/organization/usage/completions";
const COSTS_PATH: &str = "/v1/organization/costs";
const WINDOW_SECONDS: i64 = 7 * 24 * 60 * 60;

/// Summarizes OpenAI organization usage over the recent reporting window.
#[derive(Debug, Clone, PartialEq)]
pub struct OpenAIUsageSummary {
    pub window_start_s: i64,
    pub window_end_s: i64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub input_cached_tokens: u64,
    pub num_model_requests: u64,
    pub total_cost_usd: Option<f64>,
}

/// Reports typed failures when querying OpenAI organization usage endpoints.
#[derive(Debug, Clone, Error, PartialEq, Eq)]
pub enum OpenAIUsageError {
    #[error("OpenAI usage is not supported for the current credential")]
    UnsupportedCredential,
    #[error("OpenAI usage is not supported for this provider")]
    UnsupportedProvider,
    #[error("OpenAI usage request failed with status {status}")]
    HttpStatus { status: u16 },
    #[error("failed to send OpenAI usage request: {message}")]
    Transport { message: String },
    #[error("failed to parse OpenAI usage response: {message}")]
    Parse { message: String },
}

/// Fetches recent OpenAI organization usage and costs using the provided bearer token.
pub fn fetch_usage_summary(
    base_url: &str,
    bearer_token: &str,
) -> Result<OpenAIUsageSummary, OpenAIUsageError> {
    if !supports_usage_endpoint(base_url) {
        return Err(OpenAIUsageError::UnsupportedProvider);
    }
    let client = Client::new();
    let now_s = now_unix_s();
    let start_s = now_s - WINDOW_SECONDS;
    let usage = get_json::<OpenAIUsagePage<OpenAICompletionsBucketResult>>(
        &client,
        &format!(
            "{}{}?start_time={start_s}&bucket_width=1d&limit=7",
            base_url.trim_end_matches('/'),
            COMPLETIONS_USAGE_PATH
        ),
        bearer_token,
    )?;
    let costs = get_json::<OpenAICostPage>(
        &client,
        &format!(
            "{}{}?start_time={start_s}&bucket_width=1d&limit=7",
            base_url.trim_end_matches('/'),
            COSTS_PATH
        ),
        bearer_token,
    )?;
    Ok(OpenAIUsageSummary {
        window_start_s: start_s,
        window_end_s: now_s,
        input_tokens: usage
            .data
            .iter()
            .flat_map(|bucket| bucket.results.iter())
            .map(|result| result.input_tokens)
            .sum(),
        output_tokens: usage
            .data
            .iter()
            .flat_map(|bucket| bucket.results.iter())
            .map(|result| result.output_tokens)
            .sum(),
        input_cached_tokens: usage
            .data
            .iter()
            .flat_map(|bucket| bucket.results.iter())
            .map(|result| result.input_cached_tokens)
            .sum(),
        num_model_requests: usage
            .data
            .iter()
            .flat_map(|bucket| bucket.results.iter())
            .map(|result| result.num_model_requests)
            .sum(),
        total_cost_usd: Some(
            costs
                .data
                .iter()
                .flat_map(|bucket| bucket.results.iter())
                .filter_map(|result| result.amount.as_ref())
                .map(|amount| amount.value)
                .sum(),
        ),
    })
}

fn supports_usage_endpoint(base_url: &str) -> bool {
    base_url.contains("api.openai.com")
        || base_url.starts_with("http://127.0.0.1:")
        || base_url.starts_with("http://localhost:")
}

fn get_json<T>(client: &Client, url: &str, bearer_token: &str) -> Result<T, OpenAIUsageError>
where
    T: for<'de> Deserialize<'de>,
{
    let response = client
        .get(url)
        .header("Authorization", format!("Bearer {bearer_token}"))
        .header("Content-Type", "application/json")
        .send()
        .map_err(|error| OpenAIUsageError::Transport {
            message: error.to_string(),
        })?;
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(OpenAIUsageError::UnsupportedCredential);
    }
    if !status.is_success() {
        return Err(OpenAIUsageError::HttpStatus {
            status: status.as_u16(),
        });
    }
    response.json().map_err(|error| OpenAIUsageError::Parse {
        message: error.to_string(),
    })
}

fn now_unix_s() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[derive(Debug, Deserialize)]
struct OpenAIUsagePage<T> {
    #[serde(default)]
    data: Vec<OpenAIUsageBucket<T>>,
}

#[derive(Debug, Deserialize)]
struct OpenAIUsageBucket<T> {
    #[serde(default)]
    results: Vec<T>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAICompletionsBucketResult {
    #[serde(default)]
    input_tokens: u64,
    #[serde(default)]
    output_tokens: u64,
    #[serde(default)]
    input_cached_tokens: u64,
    #[serde(default)]
    num_model_requests: u64,
}

#[derive(Debug, Deserialize)]
struct OpenAICostPage {
    #[serde(default)]
    data: Vec<OpenAIUsageBucket<OpenAICostBucketResult>>,
}

#[derive(Debug, Default, Deserialize)]
struct OpenAICostBucketResult {
    amount: Option<OpenAICostAmount>,
}

#[derive(Debug, Deserialize)]
struct OpenAICostAmount {
    value: f64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn fetch_usage_summary_uses_usage_and_cost_endpoints() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || -> Vec<String> {
            let mut requests = Vec::new();
            for _ in 0..2 {
                let (mut stream, _) = listener.accept().expect("connection");
                let mut buffer = [0_u8; 8192];
                let size = stream.read(&mut buffer).expect("request");
                let request = String::from_utf8_lossy(&buffer[..size]).to_string();
                let path = request.lines().next().unwrap_or_default();
                let body = if path.contains(COMPLETIONS_USAGE_PATH) {
                    serde_json::json!({
                        "data": [{
                            "results": [{
                                "input_tokens": 1200,
                                "output_tokens": 340,
                                "input_cached_tokens": 56,
                                "num_model_requests": 7
                            }]
                        }]
                    })
                    .to_string()
                } else {
                    serde_json::json!({
                        "data": [{
                            "results": [{
                                "amount": { "value": 4.25 }
                            }]
                        }]
                    })
                    .to_string()
                };
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream
                    .write_all(response.as_bytes())
                    .expect("response write");
                requests.push(request);
            }
            requests
        });

        let summary =
            fetch_usage_summary(&format!("http://{address}"), "admin-or-oauth-token").expect("usage");
        assert_eq!(summary.input_tokens, 1200);
        assert_eq!(summary.output_tokens, 340);
        assert_eq!(summary.input_cached_tokens, 56);
        assert_eq!(summary.num_model_requests, 7);
        assert_eq!(summary.total_cost_usd, Some(4.25));

        let requests = server.join().expect("join");
        assert!(requests
            .iter()
            .any(|request| request.contains(&format!("GET {COMPLETIONS_USAGE_PATH}?"))));
        assert!(requests
            .iter()
            .any(|request| request.contains(&format!("GET {COSTS_PATH}?"))));
        let lowercase = requests.join("\n").to_ascii_lowercase();
        assert!(lowercase.contains("authorization: bearer admin-or-oauth-token"));
    }

    #[test]
    fn fetch_usage_summary_classifies_unauthorized_as_unsupported() {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("connection");
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer).expect("request");
            let response = "HTTP/1.1 403 Forbidden\r\ncontent-type: application/json\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{}";
            stream
                .write_all(response.as_bytes())
                .expect("response write");
        });

        let error =
            fetch_usage_summary(&format!("http://{address}"), "rejected-token").unwrap_err();
        assert_eq!(error, OpenAIUsageError::UnsupportedCredential);
        server.join().expect("join");
    }
}
