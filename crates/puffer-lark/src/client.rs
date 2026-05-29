//! Blocking Lark OpenAPI client.

use crate::{LarkAuthKind, LarkBrand, LarkCredential};
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::multipart::{Form, Part};
use reqwest::blocking::Client;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::fs;
use std::path::Path;
use std::time::Duration;
use url::form_urlencoded;

/// Subset of auth-check data returned to callers and persisted in credentials.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LarkAuthTest {
    /// Whether Lark accepted the credential.
    pub ok: bool,
    /// Endpoint brand used for the check.
    pub brand: LarkBrand,
    /// App id when known.
    #[serde(default)]
    pub app_id: Option<String>,
    /// Tenant key when returned by the API.
    #[serde(default)]
    pub tenant_key: Option<String>,
    /// User open id when a user token was checked.
    #[serde(default)]
    pub user_open_id: Option<String>,
    /// User display name when a user token was checked.
    #[serde(default)]
    pub user_name: Option<String>,
}

/// Media upload flavor for Lark IM file uploads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LarkMediaKind {
    /// Infer image/file behavior from the path extension.
    Auto,
    /// Upload as a Lark IM image.
    Image,
    /// Upload as a generic Lark IM file.
    File,
    /// Upload as an audio file message.
    Audio,
    /// Upload as a video/media message.
    Video,
}

/// Blocking OpenAPI client for Lark app and user-token credentials.
pub struct LarkClient {
    credential: LarkCredential,
    http: Client,
}

impl LarkClient {
    /// Builds a Lark client with a conservative request timeout.
    pub fn new(credential: LarkCredential) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .context("build Lark HTTP client")?;
        Ok(Self { credential, http })
    }

    /// Returns the credential used by this client.
    pub fn credential(&self) -> &LarkCredential {
        &self.credential
    }

    /// Checks whether Lark currently accepts this credential.
    pub fn test_auth(&self) -> Result<LarkAuthTest> {
        match &self.credential.auth {
            LarkAuthKind::App { app_id, .. } => {
                let token = self.tenant_access_token()?;
                Ok(LarkAuthTest {
                    ok: !token.access_token.is_empty(),
                    brand: self.credential.brand,
                    app_id: Some(app_id.clone()),
                    tenant_key: token.tenant_key,
                    user_open_id: None,
                    user_name: None,
                })
            }
            LarkAuthKind::UserToken { app_id, .. } => {
                let data = self.api_json_request(
                    Method::GET,
                    "/open-apis/authen/v1/user_info",
                    &[],
                    None,
                )?;
                Ok(LarkAuthTest {
                    ok: true,
                    brand: self.credential.brand,
                    app_id: app_id.clone(),
                    tenant_key: string_field(&data, "tenant_key"),
                    user_open_id: string_field(&data, "open_id"),
                    user_name: string_field(&data, "name")
                        .or_else(|| string_field(&data, "en_name")),
                })
            }
        }
    }

    /// Returns whether Lark currently accepts this credential.
    pub fn is_auth_ok(&self) -> Result<bool> {
        Ok(self.test_auth()?.ok)
    }

    /// Lists chats visible to the authenticated app or user.
    pub fn list_chats(&self, page_size: usize, page_token: Option<&str>) -> Result<Value> {
        let mut query = vec![
            ("user_id_type", "open_id".to_string()),
            ("sort_type", "ByCreateTimeAsc".to_string()),
            ("page_size", page_size.clamp(1, 100).to_string()),
        ];
        if let Some(page_token) = non_empty(page_token) {
            query.push(("page_token", page_token.to_string()));
        }
        self.api_json_request(Method::GET, "/open-apis/im/v1/chats", &query, None)
    }

    /// Searches visible group chats by keyword and optional chat type filter.
    pub fn search_chats(
        &self,
        query_text: &str,
        page_size: usize,
        page_token: Option<&str>,
        search_types: &[String],
    ) -> Result<Value> {
        let mut query = vec![("page_size", page_size.clamp(1, 100).to_string())];
        if let Some(page_token) = non_empty(page_token) {
            query.push(("page_token", page_token.to_string()));
        }
        let mut body = serde_json::Map::new();
        body.insert("query".to_string(), Value::String(query_text.to_string()));
        if !search_types.is_empty() {
            body.insert(
                "filter".to_string(),
                serde_json::json!({"search_types": search_types}),
            );
        }
        self.api_json_request(
            Method::POST,
            "/open-apis/im/v2/chats/search",
            &query,
            Some(Value::Object(body)),
        )
    }

    /// Searches Lark users by keyword, ids, or supported filters.
    pub fn search_users(
        &self,
        query_text: Option<&str>,
        user_ids: &[String],
        page_size: usize,
        has_chatted: bool,
        exclude_external_users: bool,
    ) -> Result<Value> {
        let query = vec![("page_size", page_size.clamp(1, 30).to_string())];
        let mut body = serde_json::Map::new();
        if let Some(query_text) = non_empty(query_text) {
            body.insert("query".to_string(), Value::String(query_text.to_string()));
        }
        let mut filter = serde_json::Map::new();
        if !user_ids.is_empty() {
            filter.insert(
                "user_ids".to_string(),
                Value::Array(user_ids.iter().cloned().map(Value::String).collect()),
            );
        }
        if has_chatted {
            filter.insert("has_contact".to_string(), Value::Bool(true));
        }
        if exclude_external_users {
            filter.insert("exclude_outer_contact".to_string(), Value::Bool(true));
        }
        if !filter.is_empty() {
            body.insert("filter".to_string(), Value::Object(filter));
        }
        if body.is_empty() {
            bail!("Lark search_users requires query, user_ids, or a filter");
        }
        self.api_json_request(
            Method::POST,
            "/open-apis/contact/v3/users/search",
            &query,
            Some(Value::Object(body)),
        )
    }

    /// Reads messages from a chat id.
    pub fn read_messages(
        &self,
        chat_id: &str,
        page_size: usize,
        page_token: Option<&str>,
        sort: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Value> {
        self.read_message_container(
            "chat", chat_id, page_size, page_token, sort, start_time, end_time,
        )
    }

    /// Reads messages from a thread id.
    pub fn read_thread_messages(
        &self,
        thread_id: &str,
        page_size: usize,
        page_token: Option<&str>,
        sort: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Value> {
        self.read_message_container(
            "thread", thread_id, page_size, page_token, sort, start_time, end_time,
        )
    }

    fn read_message_container(
        &self,
        container_id_type: &str,
        container_id: &str,
        page_size: usize,
        page_token: Option<&str>,
        sort: Option<&str>,
        start_time: Option<&str>,
        end_time: Option<&str>,
    ) -> Result<Value> {
        let sort_type = match sort.map(str::trim).filter(|value| !value.is_empty()) {
            Some("asc") | Some("ByCreateTimeAsc") => "ByCreateTimeAsc",
            _ => "ByCreateTimeDesc",
        };
        let mut query = vec![
            ("container_id_type", container_id_type.to_string()),
            ("container_id", container_id.to_string()),
            ("sort_type", sort_type.to_string()),
            ("page_size", page_size.clamp(1, 50).to_string()),
            ("card_msg_content_type", "raw_card_content".to_string()),
        ];
        if let Some(page_token) = non_empty(page_token) {
            query.push(("page_token", page_token.to_string()));
        }
        if let Some(start_time) = non_empty(start_time) {
            query.push(("start_time", start_time.to_string()));
        }
        if let Some(end_time) = non_empty(end_time) {
            query.push(("end_time", end_time.to_string()));
        }
        self.api_json_request(Method::GET, "/open-apis/im/v1/messages", &query, None)
    }

    /// Searches messages visible to the authenticated user.
    pub fn search_messages(
        &self,
        query_text: &str,
        page_size: usize,
        page_token: Option<&str>,
        chat_ids: &[String],
        sender_ids: &[String],
        chat_type: Option<&str>,
    ) -> Result<Value> {
        let mut query = vec![("page_size", page_size.clamp(1, 50).to_string())];
        if let Some(page_token) = non_empty(page_token) {
            query.push(("page_token", page_token.to_string()));
        }
        let mut body = serde_json::Map::new();
        body.insert("query".to_string(), Value::String(query_text.to_string()));
        let mut filter = serde_json::Map::new();
        if !chat_ids.is_empty() {
            filter.insert(
                "chat_ids".to_string(),
                Value::Array(chat_ids.iter().cloned().map(Value::String).collect()),
            );
        }
        if !sender_ids.is_empty() {
            filter.insert(
                "from_ids".to_string(),
                Value::Array(sender_ids.iter().cloned().map(Value::String).collect()),
            );
        }
        if let Some(chat_type) = non_empty(chat_type) {
            filter.insert(
                "chat_type".to_string(),
                Value::String(chat_type.to_string()),
            );
        }
        if !filter.is_empty() {
            body.insert("filter".to_string(), Value::Object(filter));
        }
        self.api_json_request(
            Method::POST,
            "/open-apis/im/v1/messages/search",
            &query,
            Some(Value::Object(body)),
        )
    }

    /// Batch fetches message details by message id.
    pub fn mget_messages(&self, message_ids: &[String]) -> Result<Value> {
        if message_ids.is_empty() {
            bail!("Lark mget_messages requires at least one message id");
        }
        let query = vec![("message_ids", message_ids.join(","))];
        self.api_json_request(Method::GET, "/open-apis/im/v1/messages/mget", &query, None)
    }

    /// Sends one Lark IM message.
    pub fn send_message(
        &self,
        receive_id: &str,
        receive_id_type: &str,
        msg_type: &str,
        content: &str,
        uuid: Option<&str>,
    ) -> Result<Value> {
        let mut body = serde_json::json!({
            "receive_id": receive_id,
            "msg_type": msg_type,
            "content": content,
        });
        if let Some(uuid) = non_empty(uuid) {
            body["uuid"] = Value::String(uuid.to_string());
        }
        let query = vec![("receive_id_type", receive_id_type.to_string())];
        self.api_json_request(
            Method::POST,
            "/open-apis/im/v1/messages",
            &query,
            Some(body),
        )
    }

    /// Replies to one Lark IM message.
    pub fn reply_message(
        &self,
        message_id: &str,
        msg_type: &str,
        content: &str,
        reply_in_thread: bool,
        uuid: Option<&str>,
    ) -> Result<Value> {
        let mut body = serde_json::json!({
            "msg_type": msg_type,
            "content": content,
        });
        if reply_in_thread {
            body["reply_in_thread"] = Value::Bool(true);
        }
        if let Some(uuid) = non_empty(uuid) {
            body["uuid"] = Value::String(uuid.to_string());
        }
        self.api_json_request(
            Method::POST,
            &format!(
                "/open-apis/im/v1/messages/{}/reply",
                encode_path_segment(message_id)
            ),
            &[],
            Some(body),
        )
    }

    /// Adds an emoji reaction to a Lark message.
    pub fn add_reaction(&self, message_id: &str, emoji_type: &str) -> Result<Value> {
        self.api_json_request(
            Method::POST,
            &format!(
                "/open-apis/im/v1/messages/{}/reactions",
                encode_path_segment(message_id)
            ),
            &[],
            Some(serde_json::json!({
                "reaction_type": {
                    "emoji_type": emoji_type,
                }
            })),
        )
    }

    /// Removes a Lark message reaction by reaction id.
    pub fn delete_reaction(&self, message_id: &str, reaction_id: &str) -> Result<Value> {
        self.api_json_request(
            Method::DELETE,
            &format!(
                "/open-apis/im/v1/messages/{}/reactions/{}",
                encode_path_segment(message_id),
                encode_path_segment(reaction_id)
            ),
            &[],
            None,
        )
    }

    /// Uploads a local image file and returns the Lark image key.
    pub fn upload_image(&self, path: &Path) -> Result<String> {
        let file_name = file_name(path);
        let bytes =
            fs::read(path).with_context(|| format!("read Lark image {}", path.display()))?;
        let part = Part::bytes(bytes).file_name(file_name);
        let form = Form::new()
            .text("image_type", "message".to_string())
            .part("image", part);
        let data = self.multipart_request("/open-apis/im/v1/images", form)?;
        data.get("image_key")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Lark image upload response missing image_key"))
    }

    /// Uploads a local file and returns the Lark file key.
    pub fn upload_file(&self, path: &Path, kind: LarkMediaKind) -> Result<String> {
        let file_name = file_name(path);
        let bytes = fs::read(path).with_context(|| format!("read Lark file {}", path.display()))?;
        let part = Part::bytes(bytes).file_name(file_name.clone());
        let form = Form::new()
            .text("file_type", file_type(path, kind))
            .text("file_name", file_name)
            .part("file", part);
        let data = self.multipart_request("/open-apis/im/v1/files", form)?;
        data.get("file_key")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Lark file upload response missing file_key"))
    }

    fn tenant_access_token(&self) -> Result<TenantToken> {
        let LarkAuthKind::App { app_id, app_secret } = &self.credential.auth else {
            bail!("tenant access token requires Lark app credentials");
        };
        let url = format!(
            "{}/open-apis/auth/v3/tenant_access_token/internal",
            self.credential.brand.open_base()
        );
        let response = self
            .http
            .post(url)
            .json(&serde_json::json!({
                "app_id": app_id,
                "app_secret": app_secret,
            }))
            .send()
            .context("send Lark tenant token request")?;
        let status = response.status();
        let payload: Value = response
            .json()
            .with_context(|| format!("decode Lark tenant token response ({status})"))?;
        if !status.is_success() {
            bail!("Lark tenant token API returned HTTP {status}: {payload}");
        }
        if payload.get("code").and_then(Value::as_i64).unwrap_or(-1) != 0 {
            bail!(
                "Lark tenant token API failed: [{}] {}",
                payload.get("code").and_then(Value::as_i64).unwrap_or(-1),
                payload
                    .get("msg")
                    .and_then(Value::as_str)
                    .unwrap_or("unknown error")
            );
        }
        let access_token = payload
            .get("tenant_access_token")
            .and_then(Value::as_str)
            .map(ToString::to_string)
            .ok_or_else(|| anyhow!("Lark tenant token response missing tenant_access_token"))?;
        Ok(TenantToken {
            access_token,
            tenant_key: string_field(&payload, "tenant_key"),
        })
    }

    fn access_token(&self) -> Result<String> {
        match &self.credential.auth {
            LarkAuthKind::App { .. } => Ok(self.tenant_access_token()?.access_token),
            LarkAuthKind::UserToken {
                user_access_token, ..
            } => Ok(user_access_token.clone()),
        }
    }

    fn api_json_request(
        &self,
        method: Method,
        path: &str,
        query: &[(&str, String)],
        body: Option<Value>,
    ) -> Result<Value> {
        let token = self.access_token()?;
        let url = self.api_url(path)?;
        let mut request = self.http.request(method.clone(), url).bearer_auth(token);
        if !query.is_empty() {
            request = request.query(query);
        }
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .with_context(|| format!("send Lark API {method} {path} request"))?;
        let status = response.status();
        let payload: Value = response
            .json()
            .with_context(|| format!("decode Lark API {method} {path} response ({status})"))?;
        if !status.is_success() {
            bail!("Lark API {method} {path} returned HTTP {status}: {payload}");
        }
        validate_lark_data(method.as_str(), path, payload)
    }

    fn multipart_request(&self, path: &str, form: Form) -> Result<Value> {
        let token = self.access_token()?;
        let response = self
            .http
            .post(self.api_url(path)?)
            .bearer_auth(token)
            .multipart(form)
            .send()
            .with_context(|| format!("send Lark multipart {path} request"))?;
        let status = response.status();
        let payload: Value = response
            .json()
            .with_context(|| format!("decode Lark multipart {path} response ({status})"))?;
        if !status.is_success() {
            bail!("Lark multipart {path} returned HTTP {status}: {payload}");
        }
        validate_lark_data("POST", path, payload)
    }

    fn api_url(&self, path: &str) -> Result<String> {
        if !path.starts_with("/open-apis/") {
            bail!("Lark API path must start with /open-apis/: {path}");
        }
        Ok(format!("{}{}", self.credential.brand.open_base(), path))
    }
}

#[derive(Debug)]
struct TenantToken {
    access_token: String,
    tenant_key: Option<String>,
}

fn validate_lark_data(method: &str, path: &str, payload: Value) -> Result<Value> {
    match payload.get("code").and_then(Value::as_i64) {
        Some(0) => Ok(payload.get("data").cloned().unwrap_or(payload)),
        Some(code) => {
            let message = payload
                .get("msg")
                .and_then(Value::as_str)
                .unwrap_or("unknown error");
            bail!("Lark API {method} {path} failed: [{code}] {message}");
        }
        None => bail!("Lark API {method} {path} response missing code field: {payload}"),
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

fn encode_path_segment(value: &str) -> String {
    form_urlencoded::byte_serialize(value.as_bytes()).collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .unwrap_or("upload.bin")
        .to_string()
}

fn file_type(path: &Path, kind: LarkMediaKind) -> String {
    match kind {
        LarkMediaKind::Audio => "opus".to_string(),
        LarkMediaKind::Video => "mp4".to_string(),
        LarkMediaKind::Image | LarkMediaKind::File | LarkMediaKind::Auto => match path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default()
            .to_lowercase()
            .as_str()
        {
            "opus" | "ogg" => "opus",
            "mp4" | "mov" | "avi" | "mkv" | "webm" => "mp4",
            "pdf" => "pdf",
            "doc" | "docx" => "doc",
            "xls" | "xlsx" | "csv" => "xls",
            "ppt" | "pptx" => "ppt",
            _ => "stream",
        }
        .to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn envelope_validation_returns_data() {
        let data = validate_lark_data(
            "GET",
            "/open-apis/x",
            json!({"code": 0, "data": {"ok": true}}),
        )
        .unwrap();

        assert_eq!(data, json!({"ok": true}));
    }

    #[test]
    fn envelope_validation_rejects_lark_error() {
        let error = validate_lark_data("GET", "/open-apis/x", json!({"code": 999, "msg": "bad"}))
            .expect_err("non-zero code should fail");

        assert!(error.to_string().contains("[999] bad"));
    }

    #[test]
    fn file_types_match_lark_upload_values() {
        assert_eq!(
            file_type(Path::new("report.pdf"), LarkMediaKind::Auto),
            "pdf"
        );
        assert_eq!(
            file_type(Path::new("clip.webm"), LarkMediaKind::Auto),
            "mp4"
        );
        assert_eq!(
            file_type(Path::new("data.bin"), LarkMediaKind::Auto),
            "stream"
        );
        assert_eq!(
            file_type(Path::new("voice.bin"), LarkMediaKind::Audio),
            "opus"
        );
    }

    #[test]
    fn path_segments_are_encoded() {
        assert_eq!(encode_path_segment("om_a/b"), "om_a%2Fb");
    }
}
