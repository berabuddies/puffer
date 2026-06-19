use anyhow::{Context, Result};
use puffer_config::PlatformMediaConfig;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformMediaGenerateRequest {
    pub(crate) kind: String,
    pub(crate) prompt: String,
    pub(crate) count: Option<u8>,
    pub(crate) image_references: Vec<String>,
    pub(crate) parameter_overrides: BTreeMap<String, String>,
    pub(crate) turn_id: Option<String>,
    pub(crate) tool_call_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlatformMediaGenerateResult {
    pub(crate) job_id: String,
    pub(crate) asset_id: String,
    pub(crate) media_type: String,
    pub(crate) status: String,
    pub(crate) content_url: Option<String>,
    pub(crate) mime_type: Option<String>,
    pub(crate) size_bytes: Option<u64>,
    pub(crate) duration_ms: Option<u64>,
    pub(crate) provider_type: String,
    pub(crate) upstream_id: String,
    pub(crate) model_id: String,
    pub(crate) prompt: String,
    pub(crate) error: Option<String>,
}

pub(crate) fn generate_platform_media(
    config: &PlatformMediaConfig,
    request: PlatformMediaGenerateRequest,
) -> Result<PlatformMediaGenerateResult> {
    let token = std::env::var(&config.auth_token_env)
        .with_context(|| format!("{} is not set", config.auth_token_env))?;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(120))
        .build()
        .context("build platform media HTTP client")?;
    let response = client
        .post(&config.endpoint)
        .bearer_auth(token)
        .json(&request)
        .send()
        .context("call platform media endpoint")?;
    let status = response.status();
    let body = response.text().context("read platform media response")?;
    if !status.is_success() {
        anyhow::bail!("platform media endpoint returned {status}: {body}");
    }
    serde_json::from_str(&body).context("parse platform media response")
}

pub(crate) fn platform_media_tool_output(result: &PlatformMediaGenerateResult) -> Result<Value> {
    Ok(json!({
        "jobId": result.job_id,
        "assetId": result.asset_id,
        "mediaType": result.media_type,
        "status": result.status,
        "contentUrl": result.content_url,
        "mimeType": result.mime_type,
        "sizeBytes": result.size_bytes,
        "durationMs": result.duration_ms,
        "provider": result.provider_type,
        "upstreamId": result.upstream_id,
        "model": result.model_id,
        "prompt": result.prompt,
        "error": result.error,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_media_output_matches_media_generated_payload() {
        let result = PlatformMediaGenerateResult {
            job_id: "job-1".to_string(),
            asset_id: "asset-1".to_string(),
            media_type: "image".to_string(),
            status: "stored".to_string(),
            content_url: Some("/api/ai-gateway/assets/asset-1/content".to_string()),
            mime_type: Some("image/png".to_string()),
            size_bytes: Some(123),
            duration_ms: None,
            provider_type: "openai-api".to_string(),
            upstream_id: "upstream-1".to_string(),
            model_id: "gpt-image-1".to_string(),
            prompt: "chair".to_string(),
            error: None,
        };

        let output = platform_media_tool_output(&result).expect("json output");
        assert_eq!(output["assetId"], "asset-1");
        assert_eq!(output["mediaType"], "image");
        assert_eq!(output["provider"], "openai-api");
        assert_eq!(output["model"], "gpt-image-1");
    }
}
