use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::Path;
use std::time::Duration;

const DISCORD_API_BASE: &str = "https://discord.com/api/v10";
const DEFAULT_TIMEOUT_MS: u64 = 60_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscordActionInput {
    action: String,
    #[serde(default)]
    service: Option<String>,
    #[serde(default)]
    channel_id: Option<String>,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    embeds: Option<Value>,
    #[serde(default)]
    limit: Option<u32>,
}

#[derive(Debug, PartialEq, Eq)]
struct DiscordApiRequest {
    method: Method,
    endpoint: String,
    json: Option<Value>,
}

/// Executes one strongly declared Discord API action for verified Lambda skills.
pub fn execute_discord_action(_state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: DiscordActionInput =
        serde_json::from_value(input).context("invalid DiscordAction input")?;
    let request = discord_api_request(parsed)?;
    let token = discord_token()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Discord HTTP client")?;
    let url = format!("{DISCORD_API_BASE}{}", request.endpoint);
    let mut http_request = client
        .request(request.method.clone(), url)
        .header(AUTHORIZATION, format!("Bot {token}"))
        .header(USER_AGENT, "puffer-code/discord-action");
    if let Some(json_body) = request.json.as_ref() {
        http_request = http_request.json(json_body);
    }
    let response = http_request.send().context("send Discord API request")?;
    let status = response.status();
    let text = response.text().context("read Discord API response")?;
    if !status.is_success() {
        bail!(
            "Discord API `{}` failed with status {status}: {text}",
            request.endpoint
        );
    }
    let value = parse_discord_response(&request.endpoint, &text)?;
    Ok(serde_json::to_string_pretty(&json!({
        "endpoint": request.endpoint,
        "response": value
    }))?)
}

fn discord_api_request(input: DiscordActionInput) -> Result<DiscordApiRequest> {
    require_discord_service(input.service)?;
    match input.action.trim() {
        "sendEmbeds" => {
            let channel_id = required_channel_id(input.channel_id)?;
            let body = required(input.body, "body")?;
            let embeds = parse_embeds(input.embeds)?;
            Ok(DiscordApiRequest {
                method: Method::POST,
                endpoint: format!("/channels/{channel_id}/messages"),
                json: Some(json!({
                    "content": body,
                    "embeds": embeds,
                })),
            })
        }
        "readMessages" => {
            let channel_id = required_channel_id(input.channel_id)?;
            let limit = input.limit.unwrap_or(20);
            if !(1..=100).contains(&limit) {
                bail!("DiscordAction `limit` must be between 1 and 100");
            }
            Ok(DiscordApiRequest {
                method: Method::GET,
                endpoint: format!("/channels/{channel_id}/messages?limit={limit}"),
                json: None,
            })
        }
        other => bail!("unsupported DiscordAction action `{other}`"),
    }
}

fn require_discord_service(service: Option<String>) -> Result<()> {
    let Some(service) = service else {
        return Ok(());
    };
    if service.trim().eq_ignore_ascii_case("discord") {
        return Ok(());
    }
    bail!("DiscordAction `service` must be `discord`")
}

fn required(value: Option<String>, name: &str) -> Result<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .with_context(|| format!("DiscordAction `{name}` is required"))
}

fn required_channel_id(value: Option<String>) -> Result<String> {
    let channel_id = required(value, "channelId")?;
    if !channel_id.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("DiscordAction `channelId` must be a Discord channel snowflake");
    }
    Ok(channel_id)
}

fn parse_embeds(value: Option<Value>) -> Result<Value> {
    let value = value.context("DiscordAction `embeds` is required")?;
    let parsed = match value {
        Value::String(text) => serde_json::from_str::<Value>(text.trim())
            .context("DiscordAction `embeds` must be JSON")?,
        other => other,
    };
    let embeds = match parsed {
        Value::Array(items) => items,
        Value::Object(_) => vec![parsed],
        _ => bail!("DiscordAction `embeds` must be a JSON object or array"),
    };
    if embeds.is_empty() {
        bail!("DiscordAction `embeds` must include at least one embed");
    }
    if embeds.len() > 10 {
        bail!("DiscordAction supports at most 10 embeds");
    }
    if !embeds.iter().all(Value::is_object) {
        bail!("DiscordAction `embeds` entries must be JSON objects");
    }
    Ok(Value::Array(embeds))
}

fn discord_token() -> Result<String> {
    std::env::var("PUFFER_DISCORD_BOT_TOKEN")
        .or_else(|_| std::env::var("DISCORD_BOT_TOKEN"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("DiscordAction requires PUFFER_DISCORD_BOT_TOKEN or DISCORD_BOT_TOKEN")
}

fn parse_discord_response(endpoint: &str, text: &str) -> Result<Value> {
    if text.trim().is_empty() {
        return Ok(Value::Null);
    }
    serde_json::from_str::<Value>(text)
        .with_context(|| format!("Discord API `{endpoint}` returned non-JSON response"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builds_send_embeds_request() {
        let request = discord_api_request(DiscordActionInput {
            action: "sendEmbeds".to_string(),
            service: Some("discord".to_string()),
            channel_id: Some("123456789012345678".to_string()),
            body: Some("hello".to_string()),
            embeds: Some(json!(r#"[{"title":"T","description":"D"}]"#)),
            limit: None,
        })
        .unwrap();

        assert_eq!(
            request,
            DiscordApiRequest {
                method: Method::POST,
                endpoint: "/channels/123456789012345678/messages".to_string(),
                json: Some(json!({
                    "content": "hello",
                    "embeds": [{"title": "T", "description": "D"}],
                })),
            }
        );
    }

    #[test]
    fn builds_read_messages_request() {
        let request = discord_api_request(DiscordActionInput {
            action: "readMessages".to_string(),
            service: Some("Discord".to_string()),
            channel_id: Some("123456789012345678".to_string()),
            body: None,
            embeds: None,
            limit: Some(25),
        })
        .unwrap();

        assert_eq!(
            request,
            DiscordApiRequest {
                method: Method::GET,
                endpoint: "/channels/123456789012345678/messages?limit=25".to_string(),
                json: None,
            }
        );
    }

    #[test]
    fn rejects_non_discord_service() {
        let error = discord_api_request(DiscordActionInput {
            action: "readMessages".to_string(),
            service: Some("slack".to_string()),
            channel_id: Some("123456789012345678".to_string()),
            body: None,
            embeds: None,
            limit: Some(25),
        })
        .expect_err("wrong service must fail closed");

        assert!(format!("{error:#}").contains("service"));
    }

    #[test]
    fn rejects_invalid_channel_id() {
        let error = discord_api_request(DiscordActionInput {
            action: "readMessages".to_string(),
            service: Some("discord".to_string()),
            channel_id: Some("#general".to_string()),
            body: None,
            embeds: None,
            limit: Some(25),
        })
        .expect_err("non-snowflake channel ids must fail closed");

        assert!(format!("{error:#}").contains("snowflake"));
    }

    #[test]
    fn rejects_out_of_range_limit() {
        let error = discord_api_request(DiscordActionInput {
            action: "readMessages".to_string(),
            service: Some("discord".to_string()),
            channel_id: Some("123456789012345678".to_string()),
            body: None,
            embeds: None,
            limit: Some(101),
        })
        .expect_err("Discord limit must not be silently clamped");

        assert!(format!("{error:#}").contains("between 1 and 100"));
    }
}
