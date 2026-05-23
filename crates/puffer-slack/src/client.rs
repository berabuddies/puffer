//! Blocking Slack Web API client.

use crate::{SlackAuthKind, SlackCredential};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;
use url::form_urlencoded;

/// Subset of `auth.test` returned to callers and persisted in credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SlackAuthTest {
    /// Whether Slack accepted the token.
    pub ok: bool,
    /// Workspace name.
    #[serde(default)]
    pub team: Option<String>,
    /// Workspace id.
    #[serde(default)]
    pub team_id: Option<String>,
    /// Authenticated user name.
    #[serde(default)]
    pub user: Option<String>,
    /// Authenticated user id.
    #[serde(default)]
    pub user_id: Option<String>,
    /// Bot id when the token belongs to a bot.
    #[serde(default)]
    pub bot_id: Option<String>,
    /// Workspace URL when Slack returns it.
    #[serde(default)]
    pub url: Option<String>,
}

/// Blocking Slack client for app, OAuth, and local-browser-session auth.
pub struct SlackClient {
    credential: SlackCredential,
    http: Client,
}

impl SlackClient {
    /// Builds a Slack client with a conservative request timeout.
    pub fn new(credential: SlackCredential) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build Slack HTTP client")?;
        Ok(Self { credential, http })
    }

    /// Returns the credential used by this client.
    pub fn credential(&self) -> &SlackCredential {
        &self.credential
    }

    /// Calls Slack `auth.test`.
    pub fn test_auth(&self) -> Result<SlackAuthTest> {
        let payload = self.api_request("auth.test", &[])?;
        Ok(SlackAuthTest {
            ok: payload.get("ok").and_then(Value::as_bool).unwrap_or(false),
            team: string_field(&payload, "team"),
            team_id: string_field(&payload, "team_id"),
            user: string_field(&payload, "user"),
            user_id: string_field(&payload, "user_id"),
            bot_id: string_field(&payload, "bot_id"),
            url: string_field(&payload, "url"),
        })
    }

    /// Returns whether Slack currently accepts this credential.
    pub fn is_auth_ok(&self) -> Result<bool> {
        Ok(self.test_auth()?.ok)
    }

    /// Calls a Slack Web API method and verifies the standard `ok` field.
    pub fn api_request(&self, method: &str, params: &[(&str, String)]) -> Result<Value> {
        let url = self.api_url(method)?;
        let mut form = params
            .iter()
            .map(|(key, value)| ((*key).to_string(), value.clone()))
            .collect::<Vec<_>>();
        let mut request = self.http.post(url).form(&form);
        match &self.credential.auth {
            SlackAuthKind::App { bot_token, .. } => {
                request = request.bearer_auth(bot_token);
            }
            SlackAuthKind::Standard { token, .. } => {
                request = request.bearer_auth(token);
            }
            SlackAuthKind::Browser {
                workspace_url,
                xoxd_token,
                xoxc_token,
            } => {
                let workspace_url = crate::normalize_workspace_url(workspace_url)?;
                form.push(("token".to_string(), xoxc_token.clone()));
                request = self
                    .http
                    .post(self.api_url(method)?)
                    .form(&form)
                    .header("cookie", format!("d={}", cookie_value(xoxd_token)))
                    .header("origin", workspace_url.as_str())
                    .header("referer", format!("{workspace_url}/client"));
            }
        }
        let response = request.send().context("send Slack API request")?;
        let status = response.status();
        let payload: Value = response
            .json()
            .with_context(|| format!("decode Slack API {method} response ({status})"))?;
        if !status.is_success() {
            bail!("Slack API {method} returned HTTP {status}: {payload}");
        }
        validate_slack_ok(method, payload)
    }

    /// Calls `conversations.list`.
    pub fn list_conversations(
        &self,
        types: &str,
        limit: usize,
        cursor: Option<&str>,
        exclude_archived: bool,
    ) -> Result<Value> {
        let mut params = vec![
            ("types", types.to_string()),
            ("limit", limit.clamp(1, 1000).to_string()),
            ("exclude_archived", exclude_archived.to_string()),
        ];
        if let Some(cursor) = non_empty(cursor) {
            params.push(("cursor", cursor.to_string()));
        }
        self.api_request("conversations.list", &params)
    }

    /// Calls `users.list`.
    pub fn list_users(&self, limit: usize, cursor: Option<&str>) -> Result<Value> {
        let mut params = vec![("limit", limit.clamp(1, 1000).to_string())];
        if let Some(cursor) = non_empty(cursor) {
            params.push(("cursor", cursor.to_string()));
        }
        self.api_request("users.list", &params)
    }

    /// Calls `conversations.history`.
    pub fn conversation_history(
        &self,
        channel: &str,
        limit: usize,
        oldest: Option<&str>,
        latest: Option<&str>,
    ) -> Result<Value> {
        let mut params = vec![
            ("channel", channel.to_string()),
            ("limit", limit.clamp(1, 1000).to_string()),
        ];
        if let Some(oldest) = non_empty(oldest) {
            params.push(("oldest", oldest.to_string()));
        }
        if let Some(latest) = non_empty(latest) {
            params.push(("latest", latest.to_string()));
        }
        self.api_request("conversations.history", &params)
    }

    /// Calls `conversations.replies`.
    pub fn conversation_replies(
        &self,
        channel: &str,
        thread_ts: &str,
        limit: usize,
    ) -> Result<Value> {
        self.api_request(
            "conversations.replies",
            &[
                ("channel", channel.to_string()),
                ("ts", thread_ts.to_string()),
                ("limit", limit.clamp(1, 1000).to_string()),
            ],
        )
    }

    /// Calls `search.messages`.
    pub fn search_messages(
        &self,
        query: &str,
        limit: usize,
        page: Option<usize>,
        sort: Option<&str>,
        sort_dir: Option<&str>,
    ) -> Result<Value> {
        let mut params = vec![
            ("query", query.to_string()),
            ("count", limit.clamp(1, 100).to_string()),
        ];
        if let Some(page) = page {
            params.push(("page", page.max(1).to_string()));
        }
        if let Some(sort) = non_empty(sort) {
            params.push(("sort", sort.to_string()));
        }
        if let Some(sort_dir) = non_empty(sort_dir) {
            params.push(("sort_dir", sort_dir.to_string()));
        }
        self.api_request("search.messages", &params)
    }

    /// Calls `chat.postMessage`.
    pub fn post_message(
        &self,
        channel: &str,
        text: &str,
        thread_ts: Option<&str>,
    ) -> Result<Value> {
        let mut params = vec![("channel", channel.to_string()), ("text", text.to_string())];
        if let Some(thread_ts) = non_empty(thread_ts) {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        self.api_request("chat.postMessage", &params)
    }

    /// Opens or returns a direct conversation with a Slack user id.
    pub fn open_conversation(&self, user: &str) -> Result<String> {
        let payload = self.api_request("conversations.open", &[("users", user.to_string())])?;
        payload
            .get("channel")
            .and_then(|channel| channel.get("id"))
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Slack conversations.open response missing channel.id"))
    }

    /// Calls `reactions.add`.
    pub fn add_reaction(&self, channel: &str, timestamp: &str, emoji: &str) -> Result<Value> {
        self.api_request(
            "reactions.add",
            &[
                ("channel", channel.to_string()),
                ("timestamp", timestamp.to_string()),
                ("name", emoji_name(emoji)),
            ],
        )
    }

    /// Calls `reactions.remove`.
    pub fn remove_reaction(&self, channel: &str, timestamp: &str, emoji: &str) -> Result<Value> {
        self.api_request(
            "reactions.remove",
            &[
                ("channel", channel.to_string()),
                ("timestamp", timestamp.to_string()),
                ("name", emoji_name(emoji)),
            ],
        )
    }

    /// Uploads a local file to a channel using Slack's external upload flow.
    pub fn upload_file(
        &self,
        channel: &str,
        path: &Path,
        initial_comment: Option<&str>,
        thread_ts: Option<&str>,
    ) -> Result<Value> {
        let metadata = fs::metadata(path)
            .with_context(|| format!("read Slack upload file metadata {}", path.display()))?;
        if !metadata.is_file() {
            bail!("Slack upload path is not a file: {}", path.display());
        }
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .unwrap_or("upload.bin");
        let upload = self.api_request(
            "files.getUploadURLExternal",
            &[
                ("filename", filename.to_string()),
                ("length", metadata.len().to_string()),
            ],
        )?;
        let upload_url = upload
            .get("upload_url")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Slack upload response missing upload_url"))?;
        let file_id = upload
            .get("file_id")
            .and_then(Value::as_str)
            .ok_or_else(|| anyhow!("Slack upload response missing file_id"))?;
        let bytes =
            fs::read(path).with_context(|| format!("read Slack upload file {}", path.display()))?;
        let upload_response = self
            .http
            .post(upload_url)
            .header("content-type", "application/octet-stream")
            .body(bytes)
            .send()
            .context("send Slack upload bytes")?;
        if !upload_response.status().is_success() {
            bail!(
                "Slack upload URL returned HTTP {}",
                upload_response.status()
            );
        }
        let files = serde_json::json!([{"id": file_id, "title": filename}]).to_string();
        let mut params = vec![("files", files), ("channel_id", channel.to_string())];
        if let Some(initial_comment) = non_empty(initial_comment) {
            params.push(("initial_comment", initial_comment.to_string()));
        }
        if let Some(thread_ts) = non_empty(thread_ts) {
            params.push(("thread_ts", thread_ts.to_string()));
        }
        self.api_request("files.completeUploadExternal", &params)
    }

    fn api_url(&self, method: &str) -> Result<String> {
        let method = method.trim();
        if method.is_empty() {
            bail!("Slack API method is empty");
        }
        match &self.credential.auth {
            SlackAuthKind::Browser { workspace_url, .. } => {
                let workspace_url = crate::normalize_workspace_url(workspace_url)?;
                Ok(format!("{workspace_url}/api/{method}"))
            }
            SlackAuthKind::App { .. } | SlackAuthKind::Standard { .. } => {
                Ok(format!("https://slack.com/api/{method}"))
            }
        }
    }
}

fn validate_slack_ok(method: &str, payload: Value) -> Result<Value> {
    match payload.get("ok").and_then(Value::as_bool) {
        Some(true) => Ok(payload),
        Some(false) => {
            let error = payload
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("unknown_error");
            bail!("Slack API {method} failed: {error}");
        }
        None => bail!("Slack API {method} response missing ok field: {payload}"),
    }
}

fn string_field(payload: &Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

fn non_empty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn cookie_value(value: &str) -> String {
    let decoded = percent_decode_token(value);
    form_urlencoded::byte_serialize(decoded.as_bytes()).collect()
}

fn emoji_name(value: &str) -> String {
    value.trim().trim_matches(':').to_string()
}

fn percent_decode_token(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0usize;
    while index < bytes.len() {
        if bytes[index] == b'%' && index + 2 < bytes.len() {
            if let Some(byte) = hex_pair(bytes[index + 1], bytes[index + 2]) {
                decoded.push(byte);
                index += 3;
                continue;
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn hex_pair(high: u8, low: u8) -> Option<u8> {
    Some(hex_value(high)? << 4 | hex_value(low)?)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{SlackAuthKind, SlackCredential};

    #[test]
    fn browser_api_url_uses_workspace_origin() {
        let client = SlackClient::new(SlackCredential {
            connection_slug: "work".into(),
            connector_slug: "slack-login".into(),
            workspace_id: None,
            workspace_name: None,
            user_id: None,
            user_name: None,
            auth: SlackAuthKind::Browser {
                workspace_url: "acme.slack.com/".into(),
                xoxd_token: "xoxd-a".into(),
                xoxc_token: "xoxc-a".into(),
            },
        })
        .unwrap();

        assert_eq!(
            client.api_url("auth.test").unwrap(),
            "https://acme.slack.com/api/auth.test"
        );
    }

    #[test]
    fn slack_error_payload_becomes_error() {
        let error = validate_slack_ok(
            "chat.postMessage",
            serde_json::json!({"ok": false, "error": "not_in_channel"}),
        )
        .unwrap_err()
        .to_string();

        assert!(error.contains("not_in_channel"));
    }

    #[test]
    fn emoji_colons_are_removed_for_reaction_api() {
        assert_eq!(emoji_name(":white_check_mark:"), "white_check_mark");
    }

    #[test]
    fn browser_cookie_value_accepts_encoded_or_decoded_tokens() {
        assert_eq!(cookie_value("xoxd-a+b"), "xoxd-a%2Bb");
        assert_eq!(cookie_value("xoxd-a%2Bb"), "xoxd-a%2Bb");
    }
}
