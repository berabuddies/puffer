use crate::AppState;
use anyhow::{bail, Context, Result};
use puffer_config::MediaGenerationConfig;
use puffer_media::{
    generate_exact_media_with_cache, ExactMediaDiscoveryCache, ExactMediaGenerationRequest,
    ExactMediaGenerationResult,
};
use puffer_provider_registry::{AuthStore, ProviderRegistry};
use serde::{Deserialize, Deserializer};
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::fs;
use std::path::{Component, Path};

const MAX_PROMPT_CHARS: usize = 20_000;

/// Carries exact media runtime context into the VideoGeneration workflow tool.
#[derive(Debug, Clone, Copy)]
pub(crate) struct VideoGenerationMediaContext<'a> {
    pub(crate) providers: &'a ProviderRegistry,
    pub(crate) auth_store: &'a AuthStore,
    pub(crate) discovery_cache: &'a ExactMediaDiscoveryCache,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct VideoGenerationInput {
    prompt: String,
    #[serde(default)]
    image_references: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_scalar_parameters")]
    parameters: BTreeMap<String, String>,
    #[serde(default)]
    purpose: Option<String>,
}

#[derive(Debug, PartialEq, Eq)]
struct VideoRequest {
    provider: String,
    model: String,
    operation: String,
    prompt: String,
    image_references: Vec<String>,
    parameters: BTreeMap<String, String>,
    purpose: Option<String>,
}

/// Builds a text-to-video request from tool input and executes the media runtime.
pub fn execute_video_generation(
    state: &mut AppState,
    cwd: &Path,
    input: Value,
    media_context: Option<VideoGenerationMediaContext<'_>>,
) -> Result<String> {
    let parsed: VideoGenerationInput =
        serde_json::from_value(input).context("invalid VideoGeneration input")?;
    let settings = state
        .config
        .media
        .video
        .as_ref()
        .context("video media provider/model is not configured")?;
    let request = build_video_request(cwd, parsed, settings)?;
    let media_context = media_context.context("VideoGeneration media runtime is not configured")?;
    let generated = generate_exact_media_with_cache(
        media_context.providers,
        media_context.auth_store,
        cwd,
        exact_media_request(&request),
        media_context.discovery_cache,
    )?;
    video_generation_output(&generated, &request.parameters, request.purpose.as_deref())
}

fn build_video_request(
    cwd: &Path,
    input: VideoGenerationInput,
    settings: &MediaGenerationConfig,
) -> Result<VideoRequest> {
    let prompt = prompt_text(cwd, &input.prompt)?;
    let image_references = validate_video_image_references(&input.image_references)?;
    let (provider, model) = required_video_selection(settings)?;
    // Merge the persisted axis selections with any per-call overrides; the
    // runtime resolves these against the logical model's axes/variants.
    let mut parameters = settings.selections.clone();
    parameters.extend(input.parameters);
    Ok(VideoRequest {
        provider,
        model,
        operation: "generate".to_string(),
        prompt,
        image_references,
        parameters,
        purpose: input.purpose,
    })
}

fn deserialize_scalar_parameters<'de, D>(
    deserializer: D,
) -> std::result::Result<BTreeMap<String, String>, D::Error>
where
    D: Deserializer<'de>,
{
    let raw = BTreeMap::<String, Value>::deserialize(deserializer)?;
    raw.into_iter()
        .map(|(key, value)| scalar_parameter_value(&key, value).map(|value| (key, value)))
        .collect()
}

fn scalar_parameter_value<E>(key: &str, value: Value) -> std::result::Result<String, E>
where
    E: serde::de::Error,
{
    match value {
        Value::String(value) => Ok(value),
        Value::Number(value) => Ok(value.to_string()),
        Value::Bool(value) => Ok(value.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => Err(E::custom(format!(
            "VideoGeneration parameters.{key} must be a scalar string, number, or boolean"
        ))),
    }
}

fn exact_media_request(request: &VideoRequest) -> ExactMediaGenerationRequest {
    ExactMediaGenerationRequest {
        kind: "video".to_string(),
        provider_id: request.provider.clone(),
        model_id: request.model.clone(),
        operation: request.operation.clone(),
        prompt: request.prompt.clone(),
        image_references: request.image_references.clone(),
        parameters: request.parameters.clone(),
        count: None,
    }
}

pub(crate) fn validate_video_image_references(values: &[String]) -> Result<Vec<String>> {
    values
        .iter()
        .enumerate()
        .map(|(index, value)| validate_video_image_reference(index, value))
        .collect()
}

fn validate_video_image_reference(index: usize, value: &str) -> Result<String> {
    let trimmed = value.trim();
    if trimmed.is_empty() || trimmed.chars().any(char::is_whitespace) {
        bail!("{}", invalid_video_image_reference_message(index));
    }
    if let Some(asset_id) = trimmed.strip_prefix("asset://") {
        if !asset_id.is_empty() {
            return Ok(trimmed.to_string());
        }
        bail!("{}", invalid_video_image_reference_message(index));
    }
    let parsed = url::Url::parse(trimmed)
        .map_err(|_| anyhow::anyhow!("{}", invalid_video_image_reference_message(index)))?;
    if parsed.scheme() == "https" {
        return Ok(trimmed.to_string());
    }
    bail!("{}", invalid_video_image_reference_message(index))
}

fn invalid_video_image_reference_message(index: usize) -> String {
    format!("VideoGeneration imageReferences[{index}] must be an https:// or asset:// URL")
}

fn video_generation_output(
    result: &ExactMediaGenerationResult,
    parameters: &BTreeMap<String, String>,
    purpose: Option<&str>,
) -> Result<String> {
    let artifacts = result
        .artifacts
        .iter()
        .map(|artifact| {
            let mut value = json!({
                "artifactId": artifact.artifact_id,
                "index": artifact.index,
                "path": artifact.path,
                "mimeType": artifact.mime_type,
                "size": artifact.byte_count
            });
            if let Some(remote_source_url) = &artifact.remote_source_url {
                value["remoteSourceUrl"] = json!(remote_source_url);
            }
            value
        })
        .collect::<Vec<_>>();
    Ok(serde_json::to_string_pretty(&json!({
        "jobId": result.job_id,
        "kind": result.kind,
        "requestedCount": result.requested_count,
        "artifacts": artifacts,
        "provider": result.provider_id,
        "model": result.model_id,
        "status": result.status,
        "providerJobId": result.provider_job_id,
        "remoteStatus": result.remote_status,
        "error": result.error,
        "diagnostic": result.diagnostic,
        "parameters": parameters,
        "purpose": purpose
    }))?)
}

fn required_video_selection(settings: &MediaGenerationConfig) -> Result<(String, String)> {
    let provider = settings.provider_id.trim();
    let model = settings.logical_model_id.trim();
    if provider.is_empty() || model.is_empty() {
        bail!("video media provider/model is not configured");
    }
    Ok((provider.to_string(), model.to_string()))
}

fn prompt_text(cwd: &Path, value: &str) -> Result<String> {
    let text = value.trim();
    if text.is_empty() {
        bail!("VideoGeneration prompt is required");
    }
    let candidate = cwd.join(text);
    let prompt = if safe_relative_path(text) && candidate.is_file() {
        fs::read_to_string(&candidate)
            .with_context(|| format!("read VideoGeneration `prompt` {}", candidate.display()))?
    } else {
        text.to_string()
    };
    let prompt = prompt.trim();
    if prompt.is_empty() {
        bail!("VideoGeneration prompt is empty");
    }
    if prompt.chars().count() > MAX_PROMPT_CHARS {
        bail!("VideoGeneration prompt exceeds {MAX_PROMPT_CHARS} characters");
    }
    Ok(prompt.to_string())
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
    use crate::AppState;
    use indexmap::IndexMap;
    use puffer_config::MediaGenerationConfig;
    use puffer_media::MediaFailureDiagnostic;
    use puffer_provider_registry::{
        AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaExecutionDescriptor,
        MediaExecutionKind, MediaKindDescriptor, MediaModelDescriptor, MediaOperation,
        ModelDescriptor, ProviderDescriptor, ProviderMediaDescriptor, ProviderRegistry, Variant,
        Variants, WireType,
    };
    use puffer_resources::LoadedResources;
    use puffer_session_store::SessionMetadata;
    use puffer_tools::{
        ToolDefinition, ToolDisplayHints, ToolInputSchema, ToolKind, ToolMetadata, ToolPolicyHints,
        ToolRegistry,
    };
    use serde_json::json;
    use std::collections::BTreeMap;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::path::Path;
    use std::thread;
    use tempfile::tempdir;
    use uuid::Uuid;

    fn video_settings() -> MediaGenerationConfig {
        MediaGenerationConfig {
            provider_id: "relaydance".to_string(),
            logical_model_id: "doubao-seedance-2-0-720p".to_string(),
            selections: BTreeMap::from([
                ("duration_seconds".to_string(), "5".to_string()),
                ("aspect_ratio".to_string(), "16:9".to_string()),
                ("resolution".to_string(), "720p".to_string()),
            ]),
        }
    }

    fn test_state(settings: Option<MediaGenerationConfig>, cwd: &Path) -> AppState {
        let mut config = puffer_config::PufferConfig::default();
        config.media.video = settings;
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

    fn video_registry(base_url: String) -> ProviderRegistry {
        let mut registry = ProviderRegistry::new();
        registry.register(ProviderDescriptor {
            id: "relaydance".to_string(),
            display_name: "Relaydance".to_string(),
            base_url,
            default_api: "openai-completions".to_string(),
            auth_modes: vec![AuthMode::ApiKey],
            headers: IndexMap::new(),
            query_params: IndexMap::new(),
            chat_completions_path: None,
            discovery: None,
            media: Some(ProviderMediaDescriptor {
                image: None,
                video: Some(MediaKindDescriptor {
                    discovery: None,
                    execution: Some(MediaExecutionDescriptor {
                        adapter: MediaExecutionKind::RelaydanceVideo,
                        base_url: None,
                        path: "/v1/video/generations".to_string(),
                        batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                        prompt_format: Default::default(),
                    }),
                    models: vec![MediaModelDescriptor {
                        id: "doubao-seedance-2-0-720p".to_string(),
                        display_name: Some("Seedance 2.0".to_string()),
                        max_outputs: None,
                        execution: None,
                        operations: vec![MediaOperation::Generate],
                        axes: vec![
                            Axis {
                                id: "duration_seconds".to_string(),
                                label: "Duration".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec![
                                        "4".to_string(),
                                        "5".to_string(),
                                        "6".to_string(),
                                        "7".to_string(),
                                        "8".to_string(),
                                        "9".to_string(),
                                        "10".to_string(),
                                        "11".to_string(),
                                        "12".to_string(),
                                        "13".to_string(),
                                        "14".to_string(),
                                        "15".to_string(),
                                    ],
                                    default: "5".to_string(),
                                },
                                request_field: Some("seconds".to_string()),
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "resolution".to_string(),
                                label: "Resolution".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec![
                                        "480p".to_string(),
                                        "720p".to_string(),
                                        "1080p".to_string(),
                                    ],
                                    default: "720p".to_string(),
                                },
                                request_field: Some("metadata.resolution".to_string()),
                                wire_type: WireType::String,
                            },
                            Axis {
                                id: "aspect_ratio".to_string(),
                                label: "Aspect ratio".to_string(),
                                role: AxisRole::Param,
                                control: ControlKind::Enum {
                                    values: vec![
                                        "16:9".to_string(),
                                        "4:3".to_string(),
                                        "1:1".to_string(),
                                        "3:4".to_string(),
                                        "9:16".to_string(),
                                        "21:9".to_string(),
                                        "adaptive".to_string(),
                                    ],
                                    default: "16:9".to_string(),
                                },
                                request_field: Some("metadata.ratio".to_string()),
                                wire_type: WireType::String,
                            },
                        ],
                        variants: Variants::Single(Variant {
                            model_id: "doubao-seedance-2-0-720p".to_string(),
                            base_params: ::std::collections::BTreeMap::new(),
                        }),
                        media_map: None,
                    }],
                }),
            }),
            models: Vec::<ModelDescriptor>::new(),
        });
        registry
    }

    fn auth_store() -> AuthStore {
        let mut auth_store = AuthStore::default();
        auth_store.set_api_key("relaydance", "sk-test");
        auth_store
    }

    fn read_http_request(stream: &mut std::net::TcpStream) -> String {
        let mut request = Vec::new();
        let mut buffer = [0_u8; 1024];
        loop {
            let size = stream.read(&mut buffer).expect("read request");
            if size == 0 {
                break;
            }
            request.extend_from_slice(&buffer[..size]);
            let Some(header_end) = header_end(&request) else {
                continue;
            };
            let content_length = content_length(&request[..header_end]).unwrap_or(0);
            let expected_size = header_end + b"\r\n\r\n".len() + content_length;
            while request.len() < expected_size {
                let size = stream.read(&mut buffer).expect("read request body");
                if size == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..size]);
            }
            break;
        }
        String::from_utf8_lossy(&request).to_string()
    }

    fn header_end(request: &[u8]) -> Option<usize> {
        request
            .windows(b"\r\n\r\n".len())
            .position(|window| window == b"\r\n\r\n")
    }

    fn content_length(headers: &[u8]) -> Option<usize> {
        let headers = String::from_utf8_lossy(headers);
        headers.lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())
                .flatten()
        })
    }

    fn http_json_response(body: serde_json::Value) -> String {
        let body = body.to_string();
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        )
    }

    fn http_video_response(bytes: &[u8]) -> String {
        format!(
            "HTTP/1.1 200 OK\r\ncontent-type: video/mp4\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            bytes.len(),
            String::from_utf8_lossy(bytes)
        )
    }

    fn spawn_relaydance_video_server() -> (String, thread::JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
        let address = listener.local_addr().expect("address");
        let base_url = format!("http://{address}");
        let handle = thread::spawn(move || {
            let mut requests = Vec::new();
            for index in 0..3 {
                let (mut stream, _) = listener.accept().expect("request");
                let request_text = read_http_request(&mut stream);
                requests.push(request_text);
                let response = match index {
                    0 => http_json_response(json!({
                        "id": "task-1",
                        "status": "queued"
                    })),
                    1 => http_json_response(json!({
                        "id": "task-1",
                        "status": "completed",
                        "metadata": { "url": format!("http://{address}/generated.mp4") }
                    })),
                    _ => http_video_response(b"mp4-bytes"),
                };
                stream.write_all(response.as_bytes()).expect("response");
            }
            requests
        });
        (base_url, handle)
    }

    fn video_generation_tool_definition() -> ToolDefinition {
        ToolDefinition {
            id: "VideoGeneration".to_string(),
            name: "VideoGeneration".to_string(),
            description: "Generate a video".to_string(),
            handler: "runtime:workflow:video_generation".to_string(),
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

    fn assert_null_diagnostic_fields(value: &Value) {
        for key in ["providerJobId", "remoteStatus", "error", "diagnostic"] {
            assert_eq!(value.get(key), Some(&Value::Null));
        }
    }

    fn assert_workflow_output_hides_internal_fields(value: &Value) {
        for key in [
            "remoteGetUrl",
            "prompt",
            "adapter",
            "rawPayload",
            "providerResponseBody",
            "credential",
            "apiKey",
        ] {
            assert!(value.get(key).is_none(), "unexpected `{key}` in {value}");
        }
    }

    #[test]
    fn execute_rejects_missing_video_provider_model_config() {
        let dir = tempdir().unwrap();
        let registry = ProviderRegistry::new();
        let auth_store = AuthStore::default();
        let discovery_cache = ExactMediaDiscoveryCache::empty();
        let mut state = test_state(None, dir.path());

        let error = execute_video_generation(
            &mut state,
            dir.path(),
            json!({"prompt": "make a ship launch video"}),
            Some(VideoGenerationMediaContext {
                providers: &registry,
                auth_store: &auth_store,
                discovery_cache: &discovery_cache,
            }),
        )
        .unwrap_err();

        assert_eq!(
            error.to_string(),
            "video media provider/model is not configured"
        );
    }

    #[test]
    fn build_request_rejects_empty_prompt() {
        let dir = tempdir().unwrap();

        let error = build_video_request(
            dir.path(),
            VideoGenerationInput {
                prompt: "  ".to_string(),
                image_references: Vec::new(),
                parameters: BTreeMap::new(),
                purpose: None,
            },
            &video_settings(),
        )
        .unwrap_err();

        assert_eq!(error.to_string(), "VideoGeneration prompt is required");
    }

    #[test]
    fn rejects_unknown_video_generation_fields() {
        let error = serde_json::from_value::<VideoGenerationInput>(json!({
            "prompt": "make a ship launch video",
            "referenceImage": "frame.png"
        }))
        .unwrap_err();

        assert!(error.to_string().contains("referenceImage"));
    }

    #[test]
    fn defaults_missing_video_generation_parameters_to_empty_map() {
        let input = serde_json::from_value::<VideoGenerationInput>(json!({
            "prompt": "make a ship launch video"
        }))
        .unwrap();

        assert!(input.parameters.is_empty());
    }

    #[test]
    fn parses_video_generation_image_references() {
        let input = serde_json::from_value::<VideoGenerationInput>(json!({
            "prompt": "animate image 1",
            "imageReferences": [
                "https://example.com/person.png",
                "asset://approved-person"
            ]
        }))
        .unwrap();

        assert_eq!(
            input.image_references,
            vec![
                "https://example.com/person.png".to_string(),
                "asset://approved-person".to_string()
            ]
        );
    }

    #[test]
    fn validates_video_generation_image_references() {
        let values = vec![
            " https://example.com/person.png ".to_string(),
            "asset://approved-person".to_string(),
        ];

        let validated = validate_video_image_references(&values).expect("valid refs");

        assert_eq!(
            validated,
            vec![
                "https://example.com/person.png".to_string(),
                "asset://approved-person".to_string()
            ]
        );
    }

    #[test]
    fn rejects_invalid_video_generation_image_references() {
        for value in [
            "",
            "http://example.com/person.png",
            "file:///tmp/person.png",
            "person.png",
            "/tmp/person.png",
            "data:image/png;base64,AAAA",
            "asset://",
            "asset:// approved-person",
        ] {
            let error = validate_video_image_references(&[value.to_string()])
                .unwrap_err()
                .to_string();
            assert!(error.contains("imageReferences[0]"), "{value}: {error}");
            assert!(error.contains("https:// or asset://"), "{value}: {error}");
        }
    }

    #[test]
    fn accepts_scalar_video_generation_parameter_values() {
        let input = serde_json::from_value::<VideoGenerationInput>(json!({
            "prompt": "make a ship launch video",
            "parameters": {
                "duration_seconds": 5,
                "aspect_ratio": "16:9",
                "camera_fixed": false
            }
        }))
        .unwrap();

        assert_eq!(input.parameters["duration_seconds"], "5");
        assert_eq!(input.parameters["aspect_ratio"], "16:9");
        assert_eq!(input.parameters["camera_fixed"], "false");
    }

    #[test]
    fn rejects_non_scalar_video_generation_parameter_values() {
        let error = serde_json::from_value::<VideoGenerationInput>(json!({
            "prompt": "make a ship launch video",
            "parameters": {
                "duration_seconds": { "seconds": 5 }
            }
        }))
        .unwrap_err();

        assert!(error.to_string().contains("parameters.duration_seconds"));
    }

    #[test]
    fn build_request_merges_saved_parameters_and_tool_overrides() {
        let dir = tempdir().unwrap();

        let request = build_video_request(
            dir.path(),
            VideoGenerationInput {
                prompt: "make a ship launch video".to_string(),
                image_references: vec!["https://example.com/person.png".to_string()],
                parameters: BTreeMap::from([
                    ("aspect_ratio".to_string(), "9:16".to_string()),
                    ("resolution".to_string(), "1080p".to_string()),
                ]),
                purpose: Some("short launch clip".to_string()),
            },
            &video_settings(),
        )
        .unwrap();

        assert_eq!(request.provider, "relaydance");
        assert_eq!(request.model, "doubao-seedance-2-0-720p");
        assert_eq!(request.parameters["duration_seconds"], "5");
        assert_eq!(request.parameters["aspect_ratio"], "9:16");
        assert_eq!(request.parameters["resolution"], "1080p");
        assert_eq!(
            request.image_references,
            vec!["https://example.com/person.png".to_string()]
        );
        assert_eq!(request.purpose.as_deref(), Some("short launch clip"));
    }

    #[test]
    fn output_includes_failed_job_diagnostics() {
        let result = ExactMediaGenerationResult {
            job_id: "job-1".to_string(),
            requested_count: 1,
            artifacts: Vec::new(),
            kind: "video".to_string(),
            provider_id: "worldrouter".to_string(),
            model_id: "seedance-2.0-fast".to_string(),
            status: "failed".to_string(),
            provider_job_id: Some("task-123".to_string()),
            remote_status: Some("failed".to_string()),
            error: Some("The service encountered an unexpected internal error.".to_string()),
            diagnostic: Some(MediaFailureDiagnostic {
                kind: "video".to_string(),
                provider_id: "worldrouter".to_string(),
                adapter: Some("worldrouter_video".to_string()),
                model_id: Some("seedance-2.0-fast".to_string()),
                phase: Some("poll".to_string()),
                provider_job_id: Some("task-123".to_string()),
                remote_status: Some("failed".to_string()),
                http_status: None,
                provider_code: None,
                request_id: None,
                error: "The service encountered an unexpected internal error.".to_string(),
                hint: Some("Provider or upstream service returned an internal error; retry later or compare another provider.".to_string()),
            }),
        };

        let output = video_generation_output(
            &result,
            &BTreeMap::from([
                ("duration".to_string(), "5".to_string()),
                ("resolution".to_string(), "480p".to_string()),
            ]),
            None,
        )
        .unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        let object = parsed.as_object().unwrap();
        assert_eq!(object.get("providerJobId"), Some(&json!("task-123")));
        assert_eq!(object.get("remoteStatus"), Some(&json!("failed")));
        assert_eq!(
            object.get("error"),
            Some(&json!(
                "The service encountered an unexpected internal error."
            ))
        );
        assert_eq!(object["diagnostic"]["provider"], json!("worldrouter"));
        assert_eq!(object["diagnostic"]["adapter"], json!("worldrouter_video"));
        assert_eq!(object["diagnostic"]["phase"], json!("poll"));
        assert_workflow_output_hides_internal_fields(&parsed);
    }

    #[test]
    fn output_includes_null_diagnostics_when_absent() {
        let result = ExactMediaGenerationResult {
            job_id: "job-1".to_string(),
            requested_count: 1,
            artifacts: Vec::new(),
            kind: "video".to_string(),
            provider_id: "provider-1".to_string(),
            model_id: "model-1".to_string(),
            status: "succeeded".to_string(),
            provider_job_id: None,
            remote_status: None,
            error: None,
            diagnostic: None,
        };

        let output = video_generation_output(&result, &BTreeMap::new(), None).unwrap();

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_null_diagnostic_fields(&parsed);
    }

    #[test]
    fn execute_uses_exact_video_generation_and_returns_artifacts() {
        let (base_url, server) = spawn_relaydance_video_server();
        let dir = tempdir().unwrap();
        let registry = video_registry(base_url);
        let auth_store = auth_store();
        let discovery_cache = ExactMediaDiscoveryCache::empty();
        let mut state = test_state(Some(video_settings()), dir.path());

        let output = execute_video_generation(
            &mut state,
            dir.path(),
            json!({
                "prompt": "make a ship launch video",
                "parameters": { "aspect_ratio": "9:16" },
                "purpose": "short launch clip"
            }),
            Some(VideoGenerationMediaContext {
                providers: &registry,
                auth_store: &auth_store,
                discovery_cache: &discovery_cache,
            }),
        )
        .unwrap();

        let requests = server.join().expect("server");
        assert!(requests[0].starts_with("POST /v1/video/generations HTTP/1.1"));
        assert!(requests[0].contains("\"model\":\"doubao-seedance-2-0-720p\""));
        assert!(requests[0].contains("\"prompt\":\"make a ship launch video\""));
        assert!(requests[0].contains("\"seconds\":\"5\""));
        assert!(requests[0].contains("\"ratio\":\"9:16\""));
        assert!(requests[1].starts_with("GET /v1/video/generations/task-1 HTTP/1.1"));
        assert!(requests[2].starts_with("GET /generated.mp4 HTTP/1.1"));

        let parsed: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(parsed["kind"], "video");
        assert_eq!(parsed["requestedCount"], 1);
        assert_eq!(parsed["provider"], "relaydance");
        assert_eq!(parsed["model"], "doubao-seedance-2-0-720p");
        assert_eq!(parsed["status"], "succeeded");
        assert_eq!(parsed["purpose"], "short launch clip");
        let object = parsed.as_object().unwrap();
        assert_eq!(object.get("providerJobId"), Some(&json!("task-1")));
        assert_eq!(object.get("remoteStatus"), Some(&json!("completed")));
        assert_eq!(object.get("error"), Some(&serde_json::Value::Null));
        assert_workflow_output_hides_internal_fields(&parsed);
        assert_eq!(parsed["parameters"]["duration_seconds"], "5");
        assert_eq!(parsed["parameters"]["resolution"], "720p");
        assert_eq!(parsed["parameters"]["aspect_ratio"], "9:16");
        let artifacts = parsed["artifacts"].as_array().unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(artifacts[0]["index"], 0);
        assert_eq!(artifacts[0]["mimeType"], "video/mp4");
        assert_eq!(artifacts[0]["size"], 9);
        let artifact_path = std::path::PathBuf::from(artifacts[0]["path"].as_str().unwrap());
        assert_eq!(std::fs::read(&artifact_path).unwrap(), b"mp4-bytes");
        assert!(artifact_path.starts_with(dir.path().join(".puffer/media/videos")));
    }

    #[test]
    fn dispatcher_passes_media_context_to_video_generation_tool() {
        let (base_url, server) = spawn_relaydance_video_server();
        let dir = tempdir().unwrap();
        let registry = video_registry(base_url);
        let auth_store = auth_store();
        let mut state = test_state(Some(video_settings()), dir.path());
        let definition = video_generation_tool_definition();

        let result = execute_tool(
            &mut state,
            &LoadedResources::default(),
            &registry,
            &auth_store,
            &ToolRegistry::default(),
            &definition,
            dir.path(),
            &allow_all_filesystem_policy(dir.path()),
            json!({
                "prompt": "make a routed launch video",
                "parameters": { "aspect_ratio": "1:1" }
            }),
            ProviderToolContext::None,
        )
        .unwrap();

        let requests = server.join().expect("server");
        assert!(result.success);
        assert!(requests[0].starts_with("POST /v1/video/generations HTTP/1.1"));
        assert!(requests[0].contains("\"model\":\"doubao-seedance-2-0-720p\""));
        assert!(requests[0].contains("\"ratio\":\"1:1\""));
        let parsed: serde_json::Value = serde_json::from_str(&result.output.stdout).unwrap();
        assert_eq!(parsed["kind"], "video");
        assert_eq!(parsed["artifacts"].as_array().unwrap().len(), 1);
    }
}
