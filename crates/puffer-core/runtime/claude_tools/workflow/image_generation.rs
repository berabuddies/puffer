use crate::AppState;
use anyhow::{bail, Context, Result};
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use base64::Engine as _;
use reqwest::blocking::Client;
use serde::Deserialize;
use serde_json::{json, Value};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_MODEL: &str = "gpt-image-1";
const DEFAULT_TIMEOUT_MS: u64 = 300_000;
const MAX_PROMPT_CHARS: usize = 20_000;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct ImageGenerationInput {
    prompt: String,
    #[serde(default)]
    prompt_reference: Option<String>,
    #[serde(default)]
    aspect: Option<String>,
    #[serde(default)]
    output_path: Option<String>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    retry_from_error: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct ImageRequest {
    model: String,
    prompt: String,
    size: String,
    output_path: PathBuf,
    purpose: Option<String>,
    retry_from_error: Option<Value>,
}

/// Generates an image through OpenAI's image API and writes it into the workspace.
pub fn execute_image_generation(_state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: ImageGenerationInput =
        serde_json::from_value(input).context("invalid ImageGeneration input")?;
    let request = build_image_request(cwd, parsed)?;
    let api_key = openai_api_key()?;
    let client = Client::builder()
        .timeout(Duration::from_millis(DEFAULT_TIMEOUT_MS))
        .build()
        .context("build image generation HTTP client")?;
    let response = client
        .post("https://api.openai.com/v1/images/generations")
        .bearer_auth(api_key)
        .json(&json!({
            "model": request.model,
            "prompt": request.prompt,
            "size": request.size,
            "n": 1
        }))
        .send()
        .context("send image generation request")?;
    let status = response.status();
    let body = response.text().context("read image generation response")?;
    if !status.is_success() {
        bail!(
            "image generation failed with status {}: {}",
            status.as_u16(),
            body
        );
    }
    let value: Value = serde_json::from_str(&body).context("parse image generation response")?;
    let image_bytes = image_bytes_from_response(&client, &value)?;
    if let Some(parent) = request.output_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("create image output directory {}", parent.display()))?;
    }
    fs::write(&request.output_path, &image_bytes)
        .with_context(|| format!("write image output {}", request.output_path.display()))?;
    Ok(serde_json::to_string_pretty(&json!({
        "path": request.output_path,
        "size": request.size,
        "model": request.model,
        "purpose": request.purpose,
        "retryFromError": request.retry_from_error.is_some()
    }))?)
}

fn build_image_request(cwd: &Path, input: ImageGenerationInput) -> Result<ImageRequest> {
    let prompt = prompt_text(cwd, &input.prompt, input.prompt_reference.as_deref())?;
    let model = std::env::var("PUFFER_IMAGE_MODEL")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_MODEL.to_string());
    Ok(ImageRequest {
        model,
        prompt,
        size: image_size(input.aspect.as_deref())?.to_string(),
        output_path: resolve_output_path(cwd, input.output_path.as_deref())?,
        purpose: input.purpose,
        retry_from_error: input.retry_from_error,
    })
}

fn prompt_text(cwd: &Path, value: &str, reference: Option<&str>) -> Result<String> {
    let primary = prompt_fragment(cwd, value, "prompt")?;
    let Some(reference) = reference.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(primary);
    };
    let reference = prompt_fragment(cwd, reference, "promptReference")?;
    let prompt = format!("Reference prompt document:\n{reference}\n\nImage prompt:\n{primary}");
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        bail!("ImageGeneration prompt exceeds {MAX_PROMPT_CHARS} characters");
    }
    Ok(prompt)
}

fn prompt_fragment(cwd: &Path, value: &str, field: &str) -> Result<String> {
    let text = value.trim();
    if text.is_empty() {
        bail!("ImageGeneration `{field}` is required");
    }
    let candidate = cwd.join(text);
    let prompt = if safe_relative_path(text) && candidate.is_file() {
        fs::read_to_string(&candidate)
            .with_context(|| format!("read ImageGeneration `{field}` {}", candidate.display()))?
    } else {
        text.to_string()
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("ImageGeneration `{field}` is empty");
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        bail!("ImageGeneration prompt exceeds {MAX_PROMPT_CHARS} characters");
    }
    Ok(prompt.to_string())
}

fn image_size(aspect: Option<&str>) -> Result<&'static str> {
    let Some(aspect) = aspect.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok("1024x1024");
    };
    match aspect.to_ascii_lowercase().as_str() {
        "square" | "1:1" | "1024x1024" => Ok("1024x1024"),
        "landscape" | "wide" | "horizontal" | "16:9" | "3:2" | "1536x1024" => Ok("1536x1024"),
        "portrait" | "vertical" | "9:16" | "2:3" | "1024x1536" => Ok("1024x1536"),
        "auto" => Ok("auto"),
        other => bail!("unsupported ImageGeneration aspect `{other}`"),
    }
}

fn resolve_output_path(cwd: &Path, value: Option<&str>) -> Result<PathBuf> {
    let relative = value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(default_output_name);
    if !safe_relative_path(&relative) {
        bail!("ImageGeneration outputPath must be a safe relative path");
    }
    Ok(cwd.join(relative))
}

fn default_output_name() -> String {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    format!(".puffer/workflows/images/generated-{stamp}.png")
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

fn image_bytes_from_response(client: &Client, value: &Value) -> Result<Vec<u8>> {
    let first = value
        .get("data")
        .and_then(Value::as_array)
        .and_then(|items| items.first())
        .context("image generation response missing data[0]")?;
    if let Some(encoded) = first.get("b64_json").and_then(Value::as_str) {
        return BASE64_STANDARD
            .decode(encoded.trim())
            .context("decode image b64_json");
    }
    if let Some(url) = first.get("url").and_then(Value::as_str) {
        return download_image_url(client, url);
    }
    bail!("image generation response missing b64_json or url")
}

fn download_image_url(client: &Client, url: &str) -> Result<Vec<u8>> {
    let parsed = reqwest::Url::parse(url).context("image response URL must be absolute")?;
    match parsed.scheme() {
        "https" => {}
        other => bail!("unsupported image response URL scheme `{other}`"),
    }
    let response = client
        .get(parsed)
        .send()
        .context("download generated image")?;
    let status = response.status();
    if !status.is_success() {
        bail!(
            "download generated image failed with status {}",
            status.as_u16()
        );
    }
    Ok(response
        .bytes()
        .context("read generated image bytes")?
        .to_vec())
}

fn openai_api_key() -> Result<String> {
    std::env::var("OPENAI_API_KEY")
        .or_else(|_| std::env::var("PUFFER_OPENAI_API_KEY"))
        .map(|value| value.trim().to_string())
        .ok()
        .filter(|value| !value.is_empty())
        .context("ImageGeneration requires OPENAI_API_KEY or PUFFER_OPENAI_API_KEY")
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn maps_common_aspects_to_image_sizes() {
        assert_eq!(image_size(None).unwrap(), "1024x1024");
        assert_eq!(image_size(Some("landscape")).unwrap(), "1536x1024");
        assert_eq!(image_size(Some("portrait")).unwrap(), "1024x1536");
        assert_eq!(image_size(Some("auto")).unwrap(), "auto");
        assert!(image_size(Some("panorama")).is_err());
    }

    #[test]
    fn reads_prompt_from_safe_workspace_relative_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("prompt.md"), "draw a careful diagram").unwrap();

        assert_eq!(
            prompt_text(dir.path(), "prompt.md", None).unwrap(),
            "draw a careful diagram"
        );
    }

    #[test]
    fn combines_prompt_reference_with_primary_prompt() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("prompts.md"), "character guide").unwrap();

        let prompt = prompt_text(dir.path(), "panel 1 action", Some("prompts.md")).unwrap();

        assert!(prompt.contains("character guide"));
        assert!(prompt.contains("panel 1 action"));
    }

    #[test]
    fn parses_prompt_reference_from_tool_input() {
        let parsed: ImageGenerationInput = serde_json::from_value(json!({
            "prompt": "panel 1 action",
            "promptReference": "prompts.md"
        }))
        .unwrap();

        assert_eq!(parsed.prompt_reference.as_deref(), Some("prompts.md"));
    }

    #[test]
    fn rejects_unsafe_output_paths() {
        let dir = tempdir().unwrap();

        assert!(resolve_output_path(dir.path(), Some("../out.png")).is_err());
        assert!(resolve_output_path(dir.path(), Some("/tmp/out.png")).is_err());
        assert!(resolve_output_path(dir.path(), Some("images/out.png")).is_ok());
    }

    #[test]
    fn extracts_base64_image_bytes() {
        let client = Client::new();
        let value = json!({
            "data": [
                {
                    "b64_json": "AAECAw=="
                }
            ]
        });

        assert_eq!(
            image_bytes_from_response(&client, &value).unwrap(),
            vec![0, 1, 2, 3]
        );
    }

    #[test]
    fn builds_request_with_prompt_file_and_output() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("prompt.md"), "make a visual summary").unwrap();

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "prompt.md".to_string(),
                prompt_reference: None,
                aspect: Some("square".to_string()),
                output_path: Some("out/image.png".to_string()),
                purpose: Some("test".to_string()),
                retry_from_error: None,
            },
        )
        .unwrap();

        assert_eq!(request.prompt, "make a visual summary");
        assert_eq!(request.size, "1024x1024");
        assert_eq!(request.output_path, dir.path().join("out/image.png"));
    }
}
