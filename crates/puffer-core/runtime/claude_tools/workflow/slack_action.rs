use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const SLACK_API_BASE: &str = "https://slack.com/api";
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct SlackActionInput {
    action: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    message: Option<String>,
    #[serde(default)]
    text: Option<String>,
    #[serde(default)]
    thread_ts: Option<String>,
    #[serde(default)]
    user: Option<String>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
struct SlackApiRequest {
    method: &'static str,
    params: Vec<(&'static str, String)>,
}

/// Executes one strongly declared Slack API action for verified Lambda skills.
pub fn execute_slack_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: SlackActionInput =
        serde_json::from_value(input).context("invalid SlackAction input")?;
    let request = slack_api_request(parsed)?;
    let token = slack_token()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Slack HTTP client")?;
    let url = format!("{SLACK_API_BASE}/{}", request.method);
    let response = client
        .post(url)
        .bearer_auth(token)
        .form(&request.params)
        .send()
        .context("send Slack API request")?;
    let status = response.status();
    let text = response.text().context("read Slack API response")?;
    if !status.is_success() {
        bail!(
            "Slack API `{}` failed with status {status}: {text}",
            request.method
        );
    }
    let value = parse_slack_response(request.method, &text)?;
    Ok(serde_json::to_string_pretty(&value)?)
}

fn slack_api_request(input: SlackActionInput) -> Result<SlackApiRequest> {
    let action = input.action.trim();
    match action {
        "reactions" => Ok(SlackApiRequest {
            method: "reactions.get",
            params: vec![
                ("channel", required(input.channel, "channel")?),
                ("timestamp", required(input.message, "message")?),
            ],
        }),
        "readMessages" => {
            let limit = input.limit.unwrap_or(20).clamp(1, 200).to_string();
            Ok(SlackApiRequest {
                method: "conversations.history",
                params: vec![
                    ("channel", required(input.channel, "channel")?),
                    ("limit", limit),
                ],
            })
        }
        "listPins" => Ok(SlackApiRequest {
            method: "pins.list",
            params: vec![("channel", required(input.channel, "channel")?)],
        }),
        "memberInfo" => Ok(SlackApiRequest {
            method: "users.info",
            params: vec![("user", required(input.user, "user")?)],
        }),
        "emojiList" => Ok(SlackApiRequest {
            method: "emoji.list",
            params: Vec::new(),
        }),
        "sendMessage" => {
            let mut params = vec![
                ("channel", required(input.channel, "channel")?),
                ("text", required(input.text, "text")?),
            ];
            if let Some(thread_ts) = optional(input.thread_ts) {
                params.push(("thread_ts", thread_ts));
            }
            Ok(SlackApiRequest {
                method: "chat.postMessage",
                params,
            })
        }
        other => bail!("unsupported SlackAction action `{other}`"),
    }
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("SlackAction `{name}` is required"))
}

fn optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn slack_token() -> Result<String> {
    std::env::var("PUFFER_SLACK_BOT_TOKEN")
        .or_else(|_| std::env::var("SLACK_BOT_TOKEN"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("SlackAction requires PUFFER_SLACK_BOT_TOKEN or SLACK_BOT_TOKEN")
}

fn parse_slack_response(method: &str, text: &str) -> Result<Value> {
    let value = serde_json::from_str::<Value>(text)
        .with_context(|| format!("Slack API `{method}` returned non-JSON response"))?;
    if value.get("ok").and_then(Value::as_bool) == Some(false) {
        bail!(
            "Slack API `{method}` returned error: {}",
            value
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error")
        );
    }
    Ok(json!({
        "method": method,
        "response": value
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_reactions_request() {
        let request = slack_api_request(SlackActionInput {
            action: "reactions".to_string(),
            channel: Some("C123".to_string()),
            message: Some("1712023032.1234".to_string()),
            text: None,
            thread_ts: None,
            user: None,
            limit: None,
        })
        .unwrap();

        assert_eq!(
            request,
            SlackApiRequest {
                method: "reactions.get",
                params: vec![
                    ("channel", "C123".to_string()),
                    ("timestamp", "1712023032.1234".to_string())
                ],
            }
        );
    }

    #[test]
    fn clamps_history_limit() {
        let request = slack_api_request(SlackActionInput {
            action: "readMessages".to_string(),
            channel: Some("C123".to_string()),
            message: None,
            text: None,
            thread_ts: None,
            user: None,
            limit: Some(500),
        })
        .unwrap();

        assert_eq!(
            request.params,
            vec![
                ("channel", "C123".to_string()),
                ("limit", "200".to_string())
            ]
        );
    }

    #[test]
    fn builds_send_message_request() {
        let request = slack_api_request(SlackActionInput {
            action: "sendMessage".to_string(),
            channel: Some("D123".to_string()),
            message: None,
            text: Some("hello".to_string()),
            thread_ts: Some("1712023032.1234".to_string()),
            user: None,
            limit: None,
        })
        .unwrap();

        assert_eq!(
            request,
            SlackApiRequest {
                method: "chat.postMessage",
                params: vec![
                    ("channel", "D123".to_string()),
                    ("text", "hello".to_string()),
                    ("thread_ts", "1712023032.1234".to_string())
                ],
            }
        );
    }

    #[test]
    fn rejects_slack_api_error_response() {
        let error = parse_slack_response("users.info", r#"{"ok":false,"error":"not_authed"}"#)
            .expect_err("Slack API errors must fail closed");

        assert!(format!("{error:#}").contains("not_authed"));
    }
}
