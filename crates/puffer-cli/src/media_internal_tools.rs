//! Internal CLI adapters for media generation tools.

use anyhow::{Context, Result};
use clap::Args;
use puffer_tools::internal_permissions::{
    require_internal_tool_execution_from_env, InternalToolExecutionRequest,
};
use serde_json::{json, Map, Value};

/// CLI arguments for `puffer internal-tool image-generation`.
#[derive(Debug, Clone, Args)]
pub(crate) struct ImageGenerationArgs {
    /// Literal prompt text or a workspace-relative prompt file path.
    #[arg(long)]
    pub(crate) prompt: String,
    /// Number of images requested by this logical generation request.
    #[arg(long)]
    pub(crate) count: u64,
    /// Optional aspect ratio such as square, landscape, 16:9, or auto.
    #[arg(long)]
    pub(crate) aspect: Option<String>,
    /// Optional workspace-relative prompt/reference file.
    #[arg(long = "prompt-reference")]
    pub(crate) prompt_reference: Option<String>,
    /// Optional caller purpose preserved in the result metadata.
    #[arg(long)]
    pub(crate) purpose: Option<String>,
    /// Previous error payload to retry from, encoded as JSON.
    #[arg(long = "retry-from-error-json")]
    pub(crate) retry_from_error_json: Option<String>,
}

/// CLI arguments for `puffer internal-tool video-generation`.
#[derive(Debug, Clone, Args)]
pub(crate) struct VideoGenerationArgs {
    /// Literal prompt text or a workspace-relative prompt file path.
    #[arg(long)]
    pub(crate) prompt: String,
    /// Optional caller purpose preserved in the result metadata.
    #[arg(long)]
    pub(crate) purpose: Option<String>,
    /// Optional scalar video parameter overrides, encoded as JSON.
    #[arg(long = "parameters-json")]
    pub(crate) parameters_json: Option<String>,
    /// Ordered public https:// image URLs or approved asset:// references.
    #[arg(long = "image-reference")]
    pub(crate) image_references: Vec<String>,
}

/// Runs one image-generation internal CLI request through the parent runtime.
pub(crate) fn run_image_generation(args: ImageGenerationArgs) -> Result<()> {
    execute_parent_internal_tool("image-generation", image_generation_input(&args)?)
}

/// Runs one video-generation internal CLI request through the parent runtime.
pub(crate) fn run_video_generation(args: VideoGenerationArgs) -> Result<()> {
    execute_parent_internal_tool("video-generation", video_generation_input(&args)?)
}

/// Builds the workflow JSON payload for an image-generation internal request.
pub(crate) fn image_generation_input(args: &ImageGenerationArgs) -> Result<Value> {
    let mut object = Map::new();
    object.insert("prompt".to_string(), Value::String(args.prompt.clone()));
    object.insert("count".to_string(), json!(args.count));
    insert_optional_string(&mut object, "aspect", &args.aspect);
    insert_optional_string(&mut object, "promptReference", &args.prompt_reference);
    insert_optional_string(&mut object, "purpose", &args.purpose);
    if let Some(raw) = args.retry_from_error_json.as_deref() {
        object.insert(
            "retryFromError".to_string(),
            parse_json_arg("--retry-from-error-json", raw)?,
        );
    }
    Ok(Value::Object(object))
}

/// Builds the workflow JSON payload for a video-generation internal request.
pub(crate) fn video_generation_input(args: &VideoGenerationArgs) -> Result<Value> {
    let mut object = Map::new();
    object.insert("prompt".to_string(), Value::String(args.prompt.clone()));
    insert_optional_string(&mut object, "purpose", &args.purpose);
    if let Some(raw) = args.parameters_json.as_deref() {
        object.insert(
            "parameters".to_string(),
            parse_json_arg("--parameters-json", raw)?,
        );
    }
    if !args.image_references.is_empty() {
        object.insert(
            "imageReferences".to_string(),
            Value::Array(
                args.image_references
                    .iter()
                    .cloned()
                    .map(Value::String)
                    .collect(),
            ),
        );
    }
    Ok(Value::Object(object))
}

fn execute_parent_internal_tool(tool_id: &str, input: Value) -> Result<()> {
    let response = require_internal_tool_execution_from_env(InternalToolExecutionRequest {
        tool_id: tool_id.to_string(),
        input,
    })?;
    if !response.success {
        let reason = response
            .reason
            .unwrap_or_else(|| "unknown error".to_string());
        if let Some(diagnostic) = response.diagnostic {
            let diagnostic = serde_json::to_string_pretty(&serde_json::json!({
                "status": "failed",
                "diagnostic": diagnostic
            }))?;
            anyhow::bail!("{tool_id} internal tool failed: {reason}\n{diagnostic}");
        }
        anyhow::bail!("{tool_id} internal tool failed: {reason}");
    }
    if let Some(output) = response.output {
        println!("{output}");
    }
    Ok(())
}

fn insert_optional_string(object: &mut Map<String, Value>, key: &str, value: &Option<String>) {
    if let Some(value) = value {
        object.insert(key.to_string(), Value::String(value.clone()));
    }
}

fn parse_json_arg(flag: &str, raw: &str) -> Result<Value> {
    serde_json::from_str(raw).with_context(|| format!("parse {flag} as JSON"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cli_args::{Cli, Command, InternalToolCommand};
    use clap::Parser;
    use puffer_tools::internal_permissions::{
        INTERNAL_PERMISSION_ADDR_ENV, INTERNAL_PERMISSION_TOKEN_ENV,
    };
    use serde_json::json;
    use std::ffi::OsString;
    use std::sync::Mutex;

    static ENV_LOCK: Mutex<()> = Mutex::new(());

    #[test]
    fn image_generation_cli_alias_builds_workflow_input() {
        let cli = Cli::parse_from([
            "puffer",
            "internal-tool",
            "imagegen",
            "--prompt",
            "scene.md",
            "--count",
            "3",
            "--aspect",
            "16:9",
            "--prompt-reference",
            "style.md",
            "--purpose",
            "cover",
            "--retry-from-error-json",
            "{\"code\":\"rate_limit\"}",
        ]);
        let Some(Command::InternalTool {
            command: InternalToolCommand::ImageGeneration(args),
        }) = cli.subcommand
        else {
            panic!("expected image generation internal tool command");
        };

        let input = image_generation_input(&args).expect("image input");

        assert_eq!(
            input,
            json!({
                "prompt": "scene.md",
                "count": 3,
                "aspect": "16:9",
                "promptReference": "style.md",
                "purpose": "cover",
                "retryFromError": { "code": "rate_limit" }
            })
        );
    }

    #[test]
    fn video_generation_cli_builds_workflow_input() {
        let cli = Cli::parse_from([
            "puffer",
            "internal-tool",
            "video-generation",
            "--prompt",
            "clip prompt",
            "--purpose",
            "storyboard",
            "--parameters-json",
            "{\"duration_seconds\":5,\"camera_fixed\":false}",
            "--image-reference",
            "https://example.com/person.png",
            "--image-reference",
            "asset://approved-person",
        ]);
        let Some(Command::InternalTool {
            command: InternalToolCommand::VideoGeneration(args),
        }) = cli.subcommand
        else {
            panic!("expected video generation internal tool command");
        };

        let input = video_generation_input(&args).expect("video input");

        assert_eq!(
            input,
            json!({
                "prompt": "clip prompt",
                "purpose": "storyboard",
                "imageReferences": [
                    "https://example.com/person.png",
                    "asset://approved-person"
                ],
                "parameters": {
                    "duration_seconds": 5,
                    "camera_fixed": false
                }
            })
        );
    }

    #[test]
    fn media_cli_rejects_invalid_json_payloads_at_boundary() {
        let image = ImageGenerationArgs {
            prompt: "scene".to_string(),
            count: 1,
            aspect: None,
            prompt_reference: None,
            purpose: None,
            retry_from_error_json: Some("{".to_string()),
        };
        let video = VideoGenerationArgs {
            prompt: "clip".to_string(),
            purpose: None,
            parameters_json: Some("{".to_string()),
            image_references: Vec::new(),
        };

        assert!(image_generation_input(&image)
            .unwrap_err()
            .to_string()
            .contains("--retry-from-error-json"));
        assert!(video_generation_input(&video)
            .unwrap_err()
            .to_string()
            .contains("--parameters-json"));
    }

    #[test]
    fn media_cli_fails_without_parent_execution_endpoint() {
        let _guard = ENV_LOCK.lock().unwrap();
        let old_addr = std::env::var_os(INTERNAL_PERMISSION_ADDR_ENV);
        let old_token = std::env::var_os(INTERNAL_PERMISSION_TOKEN_ENV);
        std::env::remove_var(INTERNAL_PERMISSION_ADDR_ENV);
        std::env::remove_var(INTERNAL_PERMISSION_TOKEN_ENV);

        let error = run_image_generation(ImageGenerationArgs {
            prompt: "scene".to_string(),
            count: 1,
            aspect: None,
            prompt_reference: None,
            purpose: None,
            retry_from_error_json: None,
        })
        .unwrap_err();

        restore_env(INTERNAL_PERMISSION_ADDR_ENV, old_addr);
        restore_env(INTERNAL_PERMISSION_TOKEN_ENV, old_token);
        assert!(error
            .to_string()
            .contains("internal execution endpoint is required but unavailable"));
    }

    fn restore_env(key: &str, old: Option<OsString>) {
        match old {
            Some(value) => std::env::set_var(key, value),
            None => std::env::remove_var(key),
        }
    }
}
