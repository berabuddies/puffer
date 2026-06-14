use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::MediaGenerationConfig;
use puffer_media::{
    generate_exact_image_with_cache, validate_image_generation_count, ExactImageGenerationRequest,
    ExactMediaDiscoveryCache,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

const MAX_PROMPT_CHARS: usize = 20_000;

/// Carries exact media runtime context into the ImageGeneration workflow tool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct ImageGenerationMediaContext<'a> {
    pub(crate) providers: &'a ProviderRegistry,
    pub(crate) auth_store: &'a AuthStore,
    pub(crate) discovery_cache: &'a ExactMediaDiscoveryCache,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImageGenerationInput {
    prompt: String,
    #[serde(default)]
    prompt_reference: Option<String>,
    #[serde(default)]
    aspect: Option<String>,
    #[serde(default)]
    count: Option<u8>,
    #[serde(default)]
    purpose: Option<String>,
    #[serde(default)]
    retry_from_error: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct ImageRequest {
    provider: String,
    model: String,
    prompt: String,
    parameters: BTreeMap<String, String>,
    count: Option<u8>,
    purpose: Option<String>,
    retry_from_error: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
struct ImageGenerationArtifactResult {
    artifact_id: String,
    index: usize,
    path: PathBuf,
    mime_type: String,
    byte_count: u64,
    remote_source_url: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct ImageGenerationResult {
    job_id: String,
    requested_count: u8,
    artifacts: Vec<ImageGenerationArtifactResult>,
    provider: String,
    model: String,
    status: String,
    parameters: BTreeMap<String, String>,
    purpose: Option<String>,
    retry_from_error: bool,
}

/// Generates an image through the exact media runtime and returns artifact metadata.
pub fn execute_image_generation(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
    media_context: Option<ImageGenerationMediaContext<'_>>,
) -> Result<String> {
    let parsed: ImageGenerationInput =
        serde_json::from_value(input).context("invalid ImageGeneration input")?;
    let settings = state
        .config
        .media
        .image
        .as_ref()
        .context("image media provider/model is not configured")?;
    let request = build_image_request(cwd, parsed, settings)?;
    let media_context = media_context.context("ImageGeneration media runtime is not configured")?;
    let generated = generate_exact_image_with_cache(
        media_context.providers,
        media_context.auth_store,
        cwd,
        ExactImageGenerationRequest {
            provider_id: request.provider.clone(),
            model_id: request.model.clone(),
            prompt: request.prompt.clone(),
            parameters: request.parameters.clone(),
            count: request.count,
        },
        media_context.discovery_cache,
    )?;
    let artifacts = generated
        .artifacts
        .into_iter()
        .map(|artifact| ImageGenerationArtifactResult {
            artifact_id: artifact.artifact_id,
            index: artifact.index,
            path: artifact.path,
            mime_type: artifact.mime_type,
            byte_count: artifact.byte_count,
            remote_source_url: artifact.remote_source_url,
        })
        .collect();

    image_generation_output(&ImageGenerationResult {
        job_id: generated.job_id,
        requested_count: generated.requested_count,
        artifacts,
        provider: generated.provider_id,
        model: generated.model_id,
        status: generated.status,
        parameters: request.parameters,
        purpose: request.purpose,
        retry_from_error: request.retry_from_error.is_some(),
    })
}

fn build_image_request(
    cwd: &Path,
    input: ImageGenerationInput,
    settings: &MediaGenerationConfig,
) -> Result<ImageRequest> {
    let prompt = prompt_text(cwd, &input.prompt, input.prompt_reference.as_deref())?;
    let (provider, model) = required_provider_model(settings)?;
    // Persisted axis selections; the runtime resolves them against the logical
    // model's axes/variants. The adapter is derived from the capability.
    let mut parameters = settings.selections.clone();
    apply_count_override(&mut parameters, input.count)?;
    apply_aspect_parameter(&mut parameters, input.aspect.as_deref())?;
    Ok(ImageRequest {
        provider,
        model,
        prompt,
        parameters,
        count: input.count,
        purpose: input.purpose,
        retry_from_error: input.retry_from_error,
    })
}

fn required_provider_model(settings: &MediaGenerationConfig) -> Result<(String, String)> {
    let provider = settings.provider_id.trim();
    let model = settings.logical_model_id.trim();
    if provider.is_empty() || model.is_empty() {
        bail!("image media provider/model is not configured");
    }
    Ok((provider.to_string(), model.to_string()))
}

fn image_generation_output(result: &ImageGenerationResult) -> Result<String> {
    Ok(serde_json::to_string_pretty(&json!({
        "jobId": result.job_id,
        "requestedCount": result.requested_count,
        "artifacts": result.artifacts.iter().map(|artifact| {
            let mut value = json!({
                "artifactId": artifact.artifact_id,
                "index": artifact.index,
                "path": artifact.path,
                "mimeType": artifact.mime_type,
                "size": artifact.byte_count
            });
            if let Some(url) = &artifact.remote_source_url {
                value["remoteSourceUrl"] = json!(url);
            }
            value
        }).collect::<Vec<_>>(),
        "provider": result.provider,
        "model": result.model,
        "status": result.status,
        "parameters": result.parameters,
        "purpose": result.purpose,
        "retryFromError": result.retry_from_error
    }))?)
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

fn image_ratio(aspect: Option<&str>) -> Result<&'static str> {
    let Some(aspect) = aspect.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok("Auto");
    };
    match aspect.to_ascii_lowercase().as_str() {
        "square" | "1:1" | "1024x1024" => Ok("1:1"),
        "landscape" | "wide" | "horizontal" | "16:9" | "1536x1024" => Ok("16:9"),
        "portrait" | "vertical" | "9:16" | "1024x1536" => Ok("9:16"),
        "3:2" => Ok("3:2"),
        "2:3" => Ok("2:3"),
        "4:3" => Ok("4:3"),
        "3:4" => Ok("3:4"),
        "21:9" => Ok("21:9"),
        "auto" => Ok("Auto"),
        other => bail!("unsupported ImageGeneration aspect `{other}`"),
    }
}

fn apply_count_override(
    parameters: &mut BTreeMap<String, String>,
    count: Option<u8>,
) -> Result<()> {
    let Some(count) = count else {
        return Ok(());
    };
    validate_image_generation_count(count)?;
    parameters.insert("output".to_string(), count.to_string());
    Ok(())
}

fn apply_aspect_parameter(
    parameters: &mut BTreeMap<String, String>,
    aspect: Option<&str>,
) -> Result<()> {
    let Some(aspect) = aspect.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(());
    };
    parameters.insert("ratio".to_string(), image_ratio(Some(aspect))?.to_string());
    Ok(())
}

fn safe_relative_path(value: &str) -> bool {
    let path = Path::new(value);
    !path.is_absolute()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::permissions::profile::{EffectiveApprovalPolicy, EffectiveSandboxMode};
    use crate::permissions::FilesystemPermissionPolicy;
    use crate::runtime::claude_tools::{execute_tool, ProviderToolContext};
    use indexmap::IndexMap;
    use puffer_provider_registry::{
        AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaExecutionDescriptor,
        MediaExecutionKind, MediaKindDescriptor, MediaMap, MediaModelDescriptor, MediaOperation,
        MediaSizeMap, ModelDescriptor, ProviderDescriptor, ProviderMediaDescriptor,
        ProviderRegistry, Variant, Variants, WireType,
    };
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionMetadata;
    use puffer_tools::{
        ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
        ToolRegistry,
    };
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::thread;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn image_settings() -> MediaGenerationConfig {
        MediaGenerationConfig {
            provider_id: "openai".to_string(),
            logical_model_id: "gpt-image-1".to_string(),
            selections: BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "1".to_string()),
            ]),
        }
    }

    fn test_state(settings: MediaGenerationConfig, cwd: &Path) -> AppState {
        let mut config = puffer_config::PufferConfig::default();
        config.media.image = Some(settings);
        AppState::new(
            config,
            cwd.to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: cwd.to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        )
    }

    fn registry_with_provider(base_url: String) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "exact-provider".to_string(),
            display_name: "Exact Provider".to_string(),
            base_url,
            default_api: "openai-responses".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::ImagesJson,
                        base_url: None,
                        path: "/custom/images".to_string(),
                        batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                        prompt_format: Default::default(),
                    }),
                    models: vec![MediaModelDescriptor {
                        id: "exact-image-model".to_string(),
                        display_name: Some("Exact Image Model".to_string()),
                        max_outputs: Some(9),
                        execution: None,
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            Axis {
                                id: "mode".to_string(),
                                label: "Mode".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1K SD".to_string()],
                                    default: "1K SD".to_string(),
                                },
                                request_field: None,
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "ratio".to_string(),
                                label: "Ratio".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec!["1:1".to_string(), "16:9".to_string()],
                                    default: "1:1".to_string(),
                                },
                                request_field: None,
                                wire_type: WireType::String,
                            },
                        ],
                        variants: Variants::Single(Variant {
                            model_id: "exact-image-model".to_string(),
                            base_params: BTreeMap::from([
                                ("quality".to_string(), "auto".to_string()),
                                ("output_format".to_string(), "png".to_string()),
                            ]),
                        }),
                        media_map: Some(MediaMap {
                            ratio: None,
                            size: Some(MediaSizeMap {
                                field: "size".to_string(),
                                values: BTreeMap::from([(
                                    "1K SD".to_string(),
                                    BTreeMap::from([
                                        ("1:1".to_string(), Some("1024x1024".to_string())),
                                        ("16:9".to_string(), Some("1536x1024".to_string())),
                                    ]),
                                )]),
                            }),
                        }),
                    }],
                }),
                video: None,
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }

    fn auth_store() -> AuthStore {
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("exact-provider", "sk-test");
        auth_store
    }

    fn image_generation_tool_definition() -> ToolDefinition {
        ToolDefinition {
            id: "ImageGeneration".to_string(),
            name: "ImageGeneration".to_string(),
            description: "Generate an image".to_string(),
            handler: "runtime:workflow:image_generation".to_string(),
            aliases: Vec::new(),
            handler_args: Vec::new(),
            kind: ToolKind::Custom,
            input_schema: ToolInputSchema::default(),
            metadata: ToolMetadata::default(),
            policy: ToolPolicyHints::default(),
            shared_lib: None,
            enabled_if: None,
            display: ToolDisplayHints::default(),
        }
    }

    fn allow_all_filesystem_policy(root: &Path) -> FilesystemPermissionPolicy {
        FilesystemPermissionPolicy {
            approval: EffectiveApprovalPolicy::Allow,
            sandbox_mode: EffectiveSandboxMode::DangerFullAccess,
            workspace_roots: vec![root.to_path_buf()],
            session_granted: true,
            allow_all_paths: true,
        }
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut buffer = [0_u8; 8192];
        let size = stream.read(&mut buffer).expect("read request");
        String::from_utf8_lossy(&buffer[..size]).to_string()
    }

    fn spawn_image_generation_server() -> (String, thread::JoinHandle<String>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let handle = thread::spawn(move || {
            let (mut stream, _) = listener.accept().expect("request");
            let request_text = read_http_request(&mut stream);
            let body = json!({
                "data": [{"b64_json": "aW1hZ2UtYnl0ZXM="}]
            })
            .to_string();
            let response = format!(
                "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).expect("response");
            request_text
        });
        (format!("http://{address}"), handle)
    }

    #[path = "image_generation_tool_tests.rs"]
    mod tool_tests;

    #[test]
    fn maps_common_aspects_to_canonical_ratios() {
        assert_eq!(image_ratio(Some("landscape")).unwrap(), "16:9");
        assert_eq!(image_ratio(Some("portrait")).unwrap(), "9:16");
        assert_eq!(image_ratio(Some("square")).unwrap(), "1:1");
        assert_eq!(image_ratio(Some("auto")).unwrap(), "Auto");
        assert!(image_ratio(Some("panorama")).is_err());
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
            "promptReference": "prompts.md",
            "count": 1
        }))
        .unwrap();

        assert_eq!(parsed.prompt_reference.as_deref(), Some("prompts.md"));
    }

    #[test]
    fn parses_explicit_image_generation_count() {
        let parsed: ImageGenerationInput = serde_json::from_value(json!({
            "prompt": "panel 1 action",
            "promptReference": "prompts.md",
            "count": 2
        }))
        .unwrap();

        assert_eq!(parsed.prompt_reference.as_deref(), Some("prompts.md"));
        assert_eq!(parsed.count, Some(2));
    }

    #[test]
    fn parses_missing_image_generation_count_for_settings_default() {
        let parsed = serde_json::from_value::<ImageGenerationInput>(json!({
            "prompt": "panel 1 action"
        }))
        .unwrap();

        assert_eq!(parsed.count, None);
    }

    #[test]
    fn rejects_image_generation_count_outside_supported_range_at_tool_boundary() {
        let dir = tempdir().unwrap();

        for count in [0, 10] {
            let error = build_image_request(
                dir.path(),
                ImageGenerationInput {
                    prompt: "make a visual summary".to_string(),
                    prompt_reference: None,
                    aspect: None,
                    count: Some(count),
                    purpose: None,
                    retry_from_error: None,
                },
                &image_settings(),
            )
            .unwrap_err();

            assert_eq!(
                error.to_string(),
                "image generation count must be between 1 and 9"
            );
        }
    }

    #[test]
    fn rejects_unknown_image_generation_fields() {
        let error = serde_json::from_value::<ImageGenerationInput>(json!({
            "prompt": "draw a ship",
            "count": 1,
            "outputPath": "requested/ship.png"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("outputPath"));
    }

    #[test]
    fn builds_request_with_prompt_file() {
        let dir = tempdir().unwrap();
        fs::write(dir.path().join("prompt.md"), "make a visual summary").unwrap();

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "prompt.md".to_string(),
                prompt_reference: None,
                aspect: Some("square".to_string()),
                count: None,
                purpose: Some("test".to_string()),
                retry_from_error: None,
            },
            &image_settings(),
        )
        .unwrap();

        assert_eq!(request.prompt, "make a visual summary");
        assert_eq!(request.parameters["ratio"], "1:1");
    }

    #[test]
    fn builds_request_maps_aspect_to_canonical_ratio_selection() {
        let dir = tempdir().unwrap();
        let settings = MediaGenerationConfig {
            provider_id: "minimax".to_string(),
            logical_model_id: "image-01".to_string(),
            selections: BTreeMap::from([
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "1".to_string()),
            ]),
        };

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "make a visual summary".to_string(),
                prompt_reference: None,
                aspect: Some("landscape".to_string()),
                count: None,
                purpose: None,
                retry_from_error: None,
            },
            &settings,
        )
        .unwrap();

        assert_eq!(request.parameters["ratio"], "16:9");
        assert!(!request.parameters.contains_key("aspect_ratio"));
        assert!(!request.parameters.contains_key("size"));
    }

    #[test]
    fn explicit_count_overrides_persisted_output_selection() {
        let dir = tempdir().unwrap();
        let settings = MediaGenerationConfig {
            provider_id: "openai".to_string(),
            logical_model_id: "gpt-image-1".to_string(),
            selections: BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "3".to_string()),
            ]),
        };

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "make a visual summary".to_string(),
                prompt_reference: None,
                aspect: None,
                count: Some(2),
                purpose: None,
                retry_from_error: None,
            },
            &settings,
        )
        .unwrap();

        assert_eq!(request.parameters["output"], "2");
        assert_eq!(request.count, Some(2));
    }

    #[test]
    fn missing_count_keeps_persisted_output_selection() {
        let dir = tempdir().unwrap();
        let settings = MediaGenerationConfig {
            provider_id: "openai".to_string(),
            logical_model_id: "gpt-image-1".to_string(),
            selections: BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "3".to_string()),
            ]),
        };

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "make a visual summary".to_string(),
                prompt_reference: None,
                aspect: None,
                count: None,
                purpose: None,
                retry_from_error: None,
            },
            &settings,
        )
        .unwrap();

        assert_eq!(request.parameters["output"], "3");
        assert_eq!(request.count, None);
    }

    #[test]
    fn builds_request_from_media_settings_instead_of_env_model() {
        let dir = tempdir().unwrap();
        std::env::set_var("PUFFER_IMAGE_MODEL", "legacy-env-model");
        let settings = MediaGenerationConfig {
            provider_id: "openai".to_string(),
            logical_model_id: "configured-image-model".to_string(),
            selections: BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "16:9".to_string()),
                ("output".to_string(), "1".to_string()),
            ]),
        };

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "make a visual summary".to_string(),
                prompt_reference: None,
                aspect: None,
                count: None,
                purpose: None,
                retry_from_error: None,
            },
            &settings,
        )
        .unwrap();

        assert_eq!(request.provider, "openai");
        assert_eq!(request.model, "configured-image-model");
        assert_eq!(request.parameters["ratio"], "16:9");
        std::env::remove_var("PUFFER_IMAGE_MODEL");
    }

    #[test]
    fn builds_request_for_non_openai_exact_provider() {
        let dir = tempdir().unwrap();
        let settings = MediaGenerationConfig {
            provider_id: "exact-provider".to_string(),
            logical_model_id: "exact-image-model".to_string(),
            selections: BTreeMap::from([
                ("mode".to_string(), "1K SD".to_string()),
                ("ratio".to_string(), "1:1".to_string()),
                ("output".to_string(), "1".to_string()),
            ]),
        };

        let request = build_image_request(
            dir.path(),
            ImageGenerationInput {
                prompt: "make a visual summary".to_string(),
                prompt_reference: None,
                aspect: None,
                count: None,
                purpose: None,
                retry_from_error: None,
            },
            &settings,
        )
        .unwrap();

        assert_eq!(request.provider, "exact-provider");
        assert_eq!(request.model, "exact-image-model");
    }

    #[test]
    fn execute_rejects_missing_provider_model_config() {
        let dir = tempdir().unwrap();
        let registry = registry_with_provider("http://127.0.0.1:9".to_string());
        let auth_store = auth_store();
        let discovery_cache = ExactMediaDiscoveryCache::empty();
        let mut config = puffer_config::PufferConfig::default();
        config.media.image = None;
        let mut state = AppState::new(
            config,
            dir.path().to_path_buf(),
            SessionMetadata {
                id: Uuid::new_v4(),
                display_name: None,
                generated_title: None,
                cwd: dir.path().to_path_buf(),
                created_at_ms: 0,
                updated_at_ms: 0,
                parent_session_id: None,
                slug: None,
                tags: Vec::new(),
                note: None,
            },
        );

        let error = execute_image_generation(
            &mut state,
            dir.path(),
            json!({"prompt": "draw a ship", "count": 1}),
            Some(ImageGenerationMediaContext {
                providers: &registry,
                auth_store: &auth_store,
                discovery_cache: &discovery_cache,
            }),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "image media provider/model is not configured"
        );
    }

    #[test]
    fn execute_rejects_stale_exact_model_before_http() {
        let dir = tempdir().unwrap();
        let registry = registry_with_provider("http://127.0.0.1:9".to_string());
        let auth_store = auth_store();
        let discovery_cache = ExactMediaDiscoveryCache::empty();
        let mut state = test_state(
            MediaGenerationConfig {
                provider_id: "exact-provider".to_string(),
                logical_model_id: "stale-image-model".to_string(),
                selections: BTreeMap::from([
                    ("mode".to_string(), "1K SD".to_string()),
                    ("ratio".to_string(), "1:1".to_string()),
                    ("output".to_string(), "1".to_string()),
                ]),
            },
            dir.path(),
        );

        let error = execute_image_generation(
            &mut state,
            dir.path(),
            json!({"prompt": "draw a ship", "count": 1}),
            Some(ImageGenerationMediaContext {
                providers: &registry,
                auth_store: &auth_store,
                discovery_cache: &discovery_cache,
            }),
        )
        .unwrap_err();

        assert!(error.to_string().contains("stale-image-model"), "{error}");
    }

    #[test]
    fn execute_uses_descriptor_adapter_and_returns_artifacts_array() {
        let (base_url, server) = spawn_image_generation_server();
        let dir = tempdir().unwrap();
        let registry = registry_with_provider(base_url);
        let auth_store = auth_store();
        let discovery_cache = ExactMediaDiscoveryCache::empty();
        let mut state = test_state(
            MediaGenerationConfig {
                provider_id: "exact-provider".to_string(),
                logical_model_id: "exact-image-model".to_string(),
                selections: BTreeMap::from([
                    ("mode".to_string(), "1K SD".to_string()),
                    ("ratio".to_string(), "1:1".to_string()),
                    ("output".to_string(), "1".to_string()),
                ]),
            },
            dir.path(),
        );

        let output = execute_image_generation(
            &mut state,
            dir.path(),
            json!({
                "prompt": "draw a ship",
                "count": 1,
                "purpose": "test"
            }),
            Some(ImageGenerationMediaContext {
                providers: &registry,
                auth_store: &auth_store,
                discovery_cache: &discovery_cache,
            }),
        )
        .unwrap();

        let request_text = server.join().expect("server");
        assert!(request_text.starts_with("POST /custom/images HTTP/1.1"));
        assert!(request_text.contains("\"model\":\"exact-image-model\""));
        assert!(dir.path().join(".puffer/media/jobs").is_dir());
        assert!(dir.path().join(".puffer/media/artifact-sidecars").is_dir());

        let parsed: Value = serde_json::from_str(&output).unwrap();
        assert!(parsed.get("artifactId").is_none());
        assert!(parsed.get("path").is_none());
        assert_eq!(parsed["requestedCount"], 1);
        let artifact = &parsed["artifacts"][0];
        let artifact_id = artifact["artifactId"].as_str().unwrap();
        let artifact_path = PathBuf::from(artifact["path"].as_str().unwrap());
        assert_eq!(fs::read(&artifact_path).unwrap(), b"image-bytes");
        assert!(artifact_path.starts_with(dir.path().join(".puffer/media/images")));
        assert_eq!(
            artifact_path.parent().and_then(|path| path.file_name()),
            Some(std::ffi::OsStr::new(artifact_id))
        );
        assert_eq!(parsed["provider"], "exact-provider");
        assert_eq!(parsed["model"], "exact-image-model");
        assert_eq!(parsed["status"], "succeeded");
        assert_eq!(parsed["purpose"], "test");
    }

    #[test]
    fn image_output_surfaces_remote_source_url_when_present() {
        let out = image_generation_output(&ImageGenerationResult {
            job_id: "job-1".into(),
            requested_count: 1,
            artifacts: vec![ImageGenerationArtifactResult {
                artifact_id: "a1".into(),
                index: 0,
                path: PathBuf::from("p.png"),
                mime_type: "image/png".into(),
                byte_count: 3,
                remote_source_url: Some("https://img.example/p.png".into()),
            }],
            provider: "minimax".into(),
            model: "m".into(),
            status: "succeeded".into(),
            parameters: BTreeMap::new(),
            purpose: None,
            retry_from_error: false,
        })
        .unwrap();
        let parsed: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            parsed["artifacts"][0]["remoteSourceUrl"],
            "https://img.example/p.png"
        );
    }
}
