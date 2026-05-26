use crate::AppState;
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::Duration;

const DEFAULT_MODEL: &str = "gpt-4.1-mini";
const DEFAULT_TIMEOUT_MS: u64 = 120_000;
const MAX_IMAGE_BYTES: usize = 20 * 1024 * 1024;
const MAX_QUESTION_CHARS: usize = 4_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct VisionAnalyzeInput {
    image: String,
    question: String,
    #[serde(default)]
    detail: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct VisionAnalyzeRequest {
    model: String,
    image_path: PathBuf,
    image_url: String,
    question: String,
    detail: String,
}

/// Analyzes a local image with OpenAI vision and returns the text report.
pub fn execute_vision_analyze(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: VisionAnalyzeInput =
        serde_json::from_value(input).context("invalid VisionAnalyze input")?;
    let request = build_vision_request(cwd, parsed)?;
    let api_key = openai_api_key()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build vision analysis HTTP client")?;
    let response = client
        .post("https://api.openai.com/v1/responses")
        .bearer_auth(api_key)
        .json(&json!({
            "model": request.model,
            "input": [
                {
                    "role": "user",
                    "content": [
                        {
                            "type": "input_text",
                            "text": request.question
                        },
                        {
                            "type": "input_image",
                            "image_url": request.image_url,
                            "detail": request.detail
                        }
                    ]
                }
            ]
        }))
        .send()
        .context("send vision analysis request")?;
    let status = response.status();
    let body = response.text().context("read vision analysis response")?;
    if !status.is_success() {
        bail!(
            "vision analysis failed with status {}: {}",
            status.as_u16(),
            body
        );
    }
    let value: Value = serde_json::from_str(&body).context("parse vision analysis response")?;
    let text = extract_response_text(&value)?;
    Ok(text)
}

fn build_vision_request(cwd: &Path, input: VisionAnalyzeInput) -> Result<VisionAnalyzeRequest> {
    let question = input.question.trim().to_string();
    if question.is_empty() {
        bail!("VisionAnalyze `question` is required");
    }
    if question.chars().count() > MAX_QUESTION_CHARS {
        bail!("VisionAnalyze question exceeds {MAX_QUESTION_CHARS} characters");
    }
    let image_path = resolve_image_path(cwd, &input.image)?;
    let (mime_type, data) = read_image_bytes(&image_path)?;
    let detail = vision_detail(input.detail.as_deref())?;
    let model = std::env::var("PUFFER_VISION_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    Ok(VisionAnalyzeRequest {
        model,
        image_path,
        image_url: format!("data:{mime_type};base64,{}", BASE64_STANDARD.encode(data)),
        question,
        detail,
    })
}

fn resolve_image_path(cwd: &Path, value: &str) -> Result<PathBuf> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        bail!("VisionAnalyze `image` is required");
    }
    let raw = PathBuf::from(trimmed);
    let path = if raw.is_absolute() {
        raw
    } else {
        if raw.components().any(|component| {
            matches!(
                component,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        }) {
            bail!("VisionAnalyze relative image path must stay inside the workspace");
        }
        cwd.join(raw)
    };
    if !path.is_file() {
        bail!("VisionAnalyze image does not exist: {}", path.display());
    }
    path.canonicalize()
        .with_context(|| format!("canonicalize VisionAnalyze image {}", path.display()))
}

fn read_image_bytes(path: &Path) -> Result<(&'static str, Vec<u8>)> {
    let ext = path
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    let mime_type = match ext.as_str() {
        "jpg" | "jpeg" => "image/jpeg",
        "png" => "image/png",
        "gif" => "image/gif",
        "webp" => "image/webp",
        other => bail!("unsupported VisionAnalyze image extension `{other}`"),
    };
    let data =
        fs::read(path).with_context(|| format!("read VisionAnalyze image {}", path.display()))?;
    if data.is_empty() {
        bail!("VisionAnalyze image is empty");
    }
    if data.len() > MAX_IMAGE_BYTES {
        bail!("VisionAnalyze image exceeds {MAX_IMAGE_BYTES} bytes");
    }
    Ok((mime_type, data))
}

fn vision_detail(value: Option<&str>) -> Result<String> {
    let value = value.unwrap_or("auto").trim();
    match value {
        "" | "auto" => Ok("auto".to_string()),
        "low" => Ok("low".to_string()),
        "high" => Ok("high".to_string()),
        other => bail!("unsupported VisionAnalyze detail `{other}`"),
    }
}

fn extract_response_text(value: &Value) -> Result<String> {
    if let Some(text) = value
        .get("output_text")
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        return Ok(text.to_string());
    }
    if let Some(output) = value.get("output").and_then(Value::as_array) {
        let text = output
            .iter()
            .filter(|item| item.get("type").and_then(Value::as_str) == Some("message"))
            .flat_map(|item| {
                item.get("content")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
            })
            .filter(|content| content.get("type").and_then(Value::as_str) == Some("output_text"))
            .filter_map(|content| content.get("text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("");
        let text = text.trim();
        if !text.is_empty() {
            return Ok(text.to_string());
        }
    }
    bail!("vision analysis returned no text")
}

fn openai_api_key() -> Result<String> {
    std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("PUFFER_OPENAI_API_KEY"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("VisionAnalyze requires OPENAI_API_KEY or PUFFER_OPENAI_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn builds_base64_image_request() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("shot.png"), [0, 1, 2, 3]).unwrap();

        let request = build_vision_request(
            dir.path(),
            VisionAnalyzeInput {
                image: "shot.png".to_string(),
                question: "What is shown?".to_string(),
                detail: Some("low".to_string()),
            },
        )
        .unwrap();

        assert_eq!(request.detail, "low");
        assert_eq!(request.question, "What is shown?");
        assert!(request.image_url.starts_with("data:image/png;base64,"));
        assert!(request.image_url.ends_with("AAECAw=="));
    }

    #[test]
    fn rejects_relative_parent_paths() {
        let dir = tempdir().unwrap();

        let err = resolve_image_path(dir.path(), "../shot.png").unwrap_err();

        assert!(err.to_string().contains("inside the workspace"));
    }

    #[test]
    fn extracts_output_text() {
        let value = json!({
            "output": [
                {
                    "type": "message",
                    "content": [
                        {
                            "type": "output_text",
                            "text": "Device is near home."
                        }
                    ]
                }
            ]
        });

        assert_eq!(
            extract_response_text(&value).unwrap(),
            "Device is near home."
        );
    }
}
