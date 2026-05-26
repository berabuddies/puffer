use crate::AppState;
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use reqwest::header::{AUTHORIZATION, USER_AGENT};
use reqwest::Method;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeSet;
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
    guild_id: Option<String>,
    #[serde(default)]
    channel_ids: Option<Vec<String>>,
    #[serde(default)]
    query: Option<String>,
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
    if parsed.action.trim() == "searchMessages" {
        let token = discord_token()?;
        let client = discord_client()?;
        let value = execute_search_messages(&client, &token, parsed)?;
        return Ok(serde_json::to_string_pretty(&value)?);
    }
    let request = discord_api_request(parsed)?;
    let token = discord_token()?;
    let client = discord_client()?;
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
        "searchMessages" => bail!("searchMessages is executed by the search bridge"),
        other => bail!("unsupported DiscordAction action `{other}`"),
    }
}

fn execute_search_messages(
    client: &Client,
    token: &str,
    input: DiscordActionInput,
) -> Result<Value> {
    require_discord_service(input.service)?;
    let query = required(input.query, "query")?;
    let limit = input.limit.unwrap_or(20);
    if !(1..=100).contains(&limit) {
        bail!("DiscordAction `limit` must be between 1 and 100");
    }
    let channels = search_channel_ids(
        client,
        token,
        input.channel_id,
        input.channel_ids,
        input.guild_id,
    )?;
    let needle = query.to_ascii_lowercase();
    let mut matches = Vec::new();
    for channel_id in &channels {
        let endpoint = format!("/channels/{channel_id}/messages?limit=100");
        let value = discord_get(client, token, &endpoint)?;
        let Some(messages) = value.as_array() else {
            continue;
        };
        for message in filter_discord_messages(channel_id, messages, &needle) {
            matches.push(message);
            if matches.len() >= limit as usize {
                return Ok(json!({
                    "action": "searchMessages",
                    "query": query,
                    "channels": channels,
                    "matches": matches
                }));
            }
        }
    }
    Ok(json!({
        "action": "searchMessages",
        "query": query,
        "channels": channels,
        "matches": matches
    }))
}

fn search_channel_ids(
    client: &Client,
    token: &str,
    channel_id: Option<String>,
    channel_ids: Option<Vec<String>>,
    guild_id: Option<String>,
) -> Result<Vec<String>> {
    let mut ids = Vec::new();
    if let Some(channel_id) = channel_id {
        ids.push(required_channel_id(Some(channel_id))?);
    }
    if let Some(channel_ids) = channel_ids {
        for id in channel_ids {
            ids.push(required_channel_id(Some(id))?);
        }
    }
    if ids.is_empty() {
        let guild_id = required_snowflake(guild_id, "guildId")?;
        let channels = discord_get(client, token, &format!("/guilds/{guild_id}/channels"))?;
        ids.extend(guild_text_channels(&channels));
    }
    let mut seen = BTreeSet::new();
    ids.retain(|id| seen.insert(id.clone()));
    if ids.is_empty() {
        bail!("DiscordAction searchMessages needs channelId, channelIds, or a guild with text channels");
    }
    Ok(ids)
}

fn guild_text_channels(value: &Value) -> Vec<String> {
    value
        .as_array()
        .into_iter()
        .flatten()
        .filter(|channel| {
            matches!(
                channel.get("type").and_then(Value::as_i64),
                Some(0) | Some(5) | Some(10) | Some(11) | Some(12)
            )
        })
        .filter_map(|channel| channel.get("id").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

fn filter_discord_messages(channel_id: &str, messages: &[Value], needle: &str) -> Vec<Value> {
    messages
        .iter()
        .filter(|message| {
            message
                .get("content")
                .and_then(Value::as_str)
                .is_some_and(|content| content.to_ascii_lowercase().contains(needle))
        })
        .map(|message| {
            json!({
                "channelId": channel_id,
                "id": message.get("id").cloned().unwrap_or(Value::Null),
                "timestamp": message.get("timestamp").cloned().unwrap_or(Value::Null),
                "author": message.get("author").cloned().unwrap_or(Value::Null),
                "content": message.get("content").cloned().unwrap_or(Value::Null)
            })
        })
        .collect()
}

fn discord_client() -> Result<Client> {
    Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build Discord HTTP client")
}

fn discord_get(client: &Client, token: &str, endpoint: &str) -> Result<Value> {
    let url = format!("{DISCORD_API_BASE}{endpoint}");
    let response = client
        .get(url)
        .header(AUTHORIZATION, format!("Bot {token}"))
        .header(USER_AGENT, "puffer-code/discord-action")
        .send()
        .with_context(|| format!("send Discord API request `{endpoint}`"))?;
    let status = response.status();
    let text = response.text().context("read Discord API response")?;
    if !status.is_success() {
        bail!("Discord API `{endpoint}` failed with status {status}: {text}");
    }
    parse_discord_response(endpoint, &text)
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
    required_snowflake(value, "channelId")
}

fn required_snowflake(value: Option<String>, name: &str) -> Result<String> {
    let channel_id = required(value, name)?;
    if !channel_id.chars().all(|ch| ch.is_ascii_digit()) {
        bail!("DiscordAction `{name}` must be a Discord snowflake");
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
            guild_id: None,
            channel_ids: None,
            query: None,
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
            guild_id: None,
            channel_ids: None,
            query: None,
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
            guild_id: None,
            channel_ids: None,
            query: None,
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
            guild_id: None,
            channel_ids: None,
            query: None,
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
            guild_id: None,
            channel_ids: None,
            query: None,
            body: None,
            embeds: None,
            limit: Some(101),
        })
        .expect_err("Discord limit must not be silently clamped");

        assert!(format!("{error:#}").contains("between 1 and 100"));
    }

    #[test]
    fn filters_search_messages_case_insensitively() {
        let messages = vec![
            json!({"id": "1", "content": "Ship release notes", "timestamp": "t1"}),
            json!({"id": "2", "content": "unrelated", "timestamp": "t2"}),
            json!({"id": "3", "content": "release checklist", "timestamp": "t3"}),
        ];

        let matches = filter_discord_messages("123456789012345678", &messages, "release");

        assert_eq!(matches.len(), 2);
        assert_eq!(matches[0]["id"], "1");
        assert_eq!(matches[1]["id"], "3");
    }

    #[test]
    fn extracts_guild_text_channels() {
        let channels = guild_text_channels(&json!([
            {"id": "1", "type": 0},
            {"id": "2", "type": 2},
            {"id": "3", "type": 5}
        ]));

        assert_eq!(channels, vec!["1".to_string(), "3".to_string()]);
    }
}
