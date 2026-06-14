use super::*;
use indexmap::IndexMap;
use puffer_provider_registry::{
    AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaExecutionDescriptor, MediaExecutionKind,
    MediaKindDescriptor, MediaMap, MediaModelDescriptor, MediaOperation, MediaSizeMap,
    ModelDescriptor, ModelDiscoveryConfig, ProviderDescriptor, ProviderMediaDescriptor,
    ProviderRegistry, Variant, Variants, WireType,
};
use serde::Deserialize;
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use tempfile::tempdir;

#[derive(Debug, Deserialize)]
struct ProviderPack {
    id: String,
    display_name: String,
    base_url: String,
    default_api: String,
    #[serde(default)]
    auth_modes: Vec<AuthMode>,
    #[serde(default)]
    headers: IndexMap<String, String>,
    #[serde(default)]
    query_params: IndexMap<String, String>,
    #[serde(default)]
    chat_completions_path: Option<String>,
    #[serde(default)]
    discovery: Option<ModelDiscoveryConfig>,
    #[serde(default)]
    media: Option<ProviderMediaDescriptor>,
    #[serde(default)]
    models: Vec<ModelDescriptor>,
}

impl ProviderPack {
    fn into_descriptor(self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id,
            display_name: self.display_name,
            base_url: self.base_url,
            default_api: self.default_api,
            auth_modes: self.auth_modes,
            headers: self.headers,
            query_params: self.query_params,
            chat_completions_path: self.chat_completions_path,
            discovery: self.discovery,
            media: self.media,
            models: self.models,
        }
    }
}

fn minimax_registry(base_url: String) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
        id: "minimax".to_string(),
        display_name: "MiniMax".to_string(),
        base_url: "https://api.minimax.io/anthropic".to_string(),
        default_api: "anthropic-messages".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: IndexMap::new(),
        query_params: IndexMap::new(),
        chat_completions_path: None,
        discovery: None,
        media: Some(ProviderMediaDescriptor {
            image: Some(MediaKindDescriptor {
                discovery: None,
                execution: Some(MediaExecutionDescriptor {
                    adapter: MediaExecutionKind::MinimaxImage,
                    base_url: Some(base_url),
                    path: "/v1/image_generation".to_string(),
                    batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                    prompt_format: Default::default(),
                }),
                models: vec![MediaModelDescriptor {
                    id: "image-01".to_string(),
                    display_name: Some("Image 01".to_string()),
                    max_outputs: None,
                    execution: None,
                    operations: vec![MediaOperation::Generate],
                    axes: vec![
                        Axis {
                            id: "aspect_ratio".to_string(),
                            label: "Aspect ratio".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["1:1".to_string(), "16:9".to_string()],
                                default: "1:1".to_string(),
                            },
                            request_field: Some("aspect_ratio".to_string()),
                            wire_type: WireType::String,
                        },
                        Axis {
                            id: "response_format".to_string(),
                            label: "Response format".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["url".to_string(), "base64".to_string()],
                                default: "base64".to_string(),
                            },
                            request_field: Some("response_format".to_string()),
                            wire_type: WireType::String,
                        },
                    ],
                    variants: Variants::Single(Variant {
                        model_id: "image-01".to_string(),
                        base_params: ::std::collections::BTreeMap::new(),
                    }),
                    media_map: None,
                }],
            }),
            video: None,
        }),
        models: Vec::<ModelDescriptor>::new(),
    });
    registry
}

fn chat_router_registry(base_url: String) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
        id: "openrouter".to_string(),
        display_name: "OpenRouter".to_string(),
        base_url,
        default_api: "openai-completions".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: IndexMap::new(),
        query_params: IndexMap::new(),
        chat_completions_path: None,
        discovery: None,
        media: Some(ProviderMediaDescriptor {
            image: Some(MediaKindDescriptor {
                discovery: None,
                execution: Some(MediaExecutionDescriptor {
                    adapter: MediaExecutionKind::ChatImageOutput,
                    base_url: None,
                    path: "/chat/completions".to_string(),
                    batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                    prompt_format: Default::default(),
                }),
                models: Vec::new(),
            }),
            video: None,
        }),
        models: Vec::<ModelDescriptor>::new(),
    });
    registry
}

fn byteplus_seedream_registry(base_url: String) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
        id: "byteplus".to_string(),
        display_name: "BytePlus".to_string(),
        base_url,
        default_api: "openai-completions".to_string(),
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
                    path: "/images/generations".to_string(),
                    batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                    prompt_format: Default::default(),
                }),
                models: vec![MediaModelDescriptor {
                    id: "seedream-4-5-251128".to_string(),
                    display_name: Some("Seedream 4.5".to_string()),
                    max_outputs: Some(9),
                    execution: None,
                    operations: vec![MediaOperation::Generate],
                    axes: vec![
                        Axis {
                            id: "mode".to_string(),
                            label: "Mode".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["2K HD".to_string()],
                                default: "2K HD".to_string(),
                            },
                            request_field: None,
                            wire_type: WireType::String,
                        },
                        Axis {
                            id: "ratio".to_string(),
                            label: "Ratio".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["Auto".to_string()],
                                default: "Auto".to_string(),
                            },
                            request_field: None,
                            wire_type: WireType::String,
                        },
                    ],
                    variants: Variants::Single(Variant {
                        model_id: "seedream-4-5-251128".to_string(),
                        base_params: BTreeMap::from([(
                            "response_format".to_string(),
                            "b64_json".to_string(),
                        )]),
                    }),
                    media_map: Some(MediaMap {
                        ratio: None,
                        size: Some(MediaSizeMap {
                            field: "size".to_string(),
                            values: BTreeMap::from([(
                                "2K HD".to_string(),
                                BTreeMap::from([("Auto".to_string(), Some("2K".to_string()))]),
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

fn replicate_video_registry() -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
        id: "replicate".to_string(),
        display_name: "Replicate".to_string(),
        base_url: "https://api.replicate.com".to_string(),
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
                    adapter: MediaExecutionKind::ReplicateVideo,
                    base_url: None,
                    path: "/v1/predictions".to_string(),
                    batch: puffer_provider_registry::MediaBatchDescriptor::default(),
                    prompt_format: Default::default(),
                }),
                models: vec![MediaModelDescriptor {
                    id: "owner/model-version".to_string(),
                    display_name: Some("Video Model".to_string()),
                    max_outputs: None,
                    execution: None,
                    operations: vec![MediaOperation::Generate],
                    axes: vec![
                        Axis {
                            id: "aspect_ratio".to_string(),
                            label: "Aspect ratio".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["16:9".to_string(), "9:16".to_string()],
                                default: "16:9".to_string(),
                            },
                            request_field: Some("aspect_ratio".to_string()),
                            wire_type: WireType::String,
                        },
                        Axis {
                            id: "duration_seconds".to_string(),
                            label: "Duration".to_string(),
                            role: AxisRole::Param,
                            control: ControlKind::Enum {
                                values: vec!["5".to_string(), "8".to_string()],
                                default: "5".to_string(),
                            },
                            request_field: Some("duration".to_string()),
                            wire_type: WireType::String,
                        },
                    ],
                    variants: Variants::Single(Variant {
                        model_id: "owner/model-version".to_string(),
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

fn discovered_chat_image_cache() -> ExactMediaDiscoveryCache {
    ExactMediaDiscoveryCache::from_inner_for_test(
        crate::media::resolver::MediaDiscoveryCache {
            image_models: vec![crate::media::resolver::CachedImageMediaModel {
                provider_id: "openrouter".to_string(),
                model: MediaModelDescriptor {
                    id: "openrouter/image-chat".to_string(),
                    display_name: Some("Image Chat".to_string()),
                    max_outputs: None,
                    execution: None,
                    operations: vec![MediaOperation::Generate],
                    axes: Vec::new(),
                    media_map: None,
                    variants: Variants::Single(Variant {
                        model_id: "openrouter/image-chat".to_string(),
                        base_params: ::std::collections::BTreeMap::new(),
                    }),
                },
                source: "provider_discovery".to_string(),
            }],
        },
        1_000,
    )
}

fn auth_store() -> AuthStore {
    let mut auth = AuthStore::default();
    auth.set_api_key("minimax", "sk-minimax");
    auth
}

fn auth_store_for(provider_id: &str) -> AuthStore {
    let mut auth = AuthStore::default();
    auth.set_api_key(provider_id, "sk-test");
    auth
}

fn bundled_provider(provider_id: &str, yaml: &str) -> ProviderDescriptor {
    let pack: ProviderPack = serde_yaml::from_str(yaml).expect("provider yaml parses");
    assert_eq!(pack.id, provider_id);
    pack.into_descriptor()
}

fn replicate_video_runtime_fixture() -> (
    ProviderRegistry,
    AuthStore,
    ExactMediaDiscoveryCache,
    tempfile::TempDir,
) {
    (
        replicate_video_registry(),
        auth_store_for("replicate"),
        ExactMediaDiscoveryCache::empty(),
        tempdir().expect("tempdir"),
    )
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = [0_u8; 8192];
    let size = stream.read(&mut buffer).expect("read request");
    String::from_utf8_lossy(&buffer[..size]).to_string()
}

#[test]
fn exact_image_generation_rejects_invalid_count() {
    assert!(validate_image_generation_count(1).is_ok());
    assert!(validate_image_generation_count(9).is_ok());
    assert_eq!(
        validate_image_generation_count(0).unwrap_err().to_string(),
        "image generation count must be between 1 and 9"
    );
    assert_eq!(
        validate_image_generation_count(10).unwrap_err().to_string(),
        "image generation count must be between 1 and 9"
    );
}

#[test]
fn image_count_override_replaces_persisted_output_selection() {
    let parameters = image_parameters_with_count_override(
        BTreeMap::from([
            ("mode".to_string(), "1K SD".to_string()),
            ("ratio".to_string(), "1:1".to_string()),
            ("output".to_string(), "3".to_string()),
        ]),
        Some(2),
    )
    .expect("valid count override");

    assert_eq!(parameters["output"], "2");
}

#[test]
fn missing_image_count_preserves_persisted_output_selection() {
    let parameters = image_parameters_with_count_override(
        BTreeMap::from([
            ("mode".to_string(), "1K SD".to_string()),
            ("ratio".to_string(), "1:1".to_string()),
            ("output".to_string(), "3".to_string()),
        ]),
        None,
    )
    .expect("missing count is not an override");

    assert_eq!(parameters["output"], "3");
}

#[test]
fn exact_generation_result_returns_artifacts_in_order() {
    let job = MediaJob {
        id: "job-1".to_string(),
        kind: MediaKind::Image,
        provider_id: "openai".to_string(),
        model_id: "gpt-image-1".to_string(),
        adapter: Some("images_json".to_string()),
        prompt: "draw".to_string(),
        parameters: BTreeMap::from([("size".to_string(), "1024x1024".to_string())]),
        status: MediaJobStatus::Succeeded,
        provider_job_id: None,
        remote_status: None,
        remote_get_url: None,
        artifact_ids: vec!["artifact-1".to_string(), "artifact-2".to_string()],
        requested_count: 2,
        error: None,
        created_at_ms: 1,
        updated_at_ms: 2,
    };
    let artifacts = vec![
        MediaArtifact {
            id: "artifact-1".to_string(),
            job_id: "job-1".to_string(),
            kind: MediaKind::Image,
            path: PathBuf::from("/tmp/image-1.png"),
            mime_type: "image/png".to_string(),
            byte_count: 10,
            metadata: serde_json::json!({"index": 0}),
            preview: None,
            created_at_ms: 1,
        },
        MediaArtifact {
            id: "artifact-2".to_string(),
            job_id: "job-1".to_string(),
            kind: MediaKind::Image,
            path: PathBuf::from("/tmp/image-2.png"),
            mime_type: "image/png".to_string(),
            byte_count: 11,
            metadata: serde_json::json!({"index": 1}),
            preview: None,
            created_at_ms: 1,
        },
    ];

    let result = exact_generation_result(job, artifacts);

    assert_eq!(result.job_id, "job-1");
    assert_eq!(result.requested_count, 2);
    assert_eq!(result.artifacts.len(), 2);
    assert_eq!(result.artifacts[0].artifact_id, "artifact-1");
    assert_eq!(result.artifacts[0].index, 0);
    assert_eq!(result.artifacts[1].artifact_id, "artifact-2");
    assert_eq!(result.artifacts[1].index, 1);
}

fn assert_exact_media_result_hides_internal_job_fields(value: &serde_json::Value) {
    for key in [
        "remoteGetUrl",
        "prompt",
        "adapter",
        "rawPayload",
        "providerResponseBody",
    ] {
        assert!(value.get(key).is_none(), "unexpected `{key}` in {value}");
    }
}

#[test]
fn exact_media_generation_result_returns_failed_video_diagnostics() {
    let job = MediaJob {
        id: "job-1".to_string(),
        kind: MediaKind::Video,
        provider_id: "worldrouter".to_string(),
        model_id: "seedance-2.0-fast".to_string(),
        adapter: Some("worldrouter_video".to_string()),
        prompt: "make a robot battle video".to_string(),
        parameters: BTreeMap::from([
            ("duration".to_string(), "5".to_string()),
            ("resolution".to_string(), "480p".to_string()),
        ]),
        status: MediaJobStatus::Failed,
        provider_job_id: Some("task-123".to_string()),
        remote_status: Some("failed".to_string()),
        remote_get_url: Some("https://provider.example.test/tasks/task-123".to_string()),
        artifact_ids: Vec::new(),
        requested_count: 1,
        error: Some("The service encountered an unexpected internal error.".to_string()),
        created_at_ms: 1,
        updated_at_ms: 2,
    };

    let result = exact_media_generation_result(job, Vec::new());

    assert_eq!(result.status, "failed");
    assert_eq!(result.provider_job_id.as_deref(), Some("task-123"));
    assert_eq!(result.remote_status.as_deref(), Some("failed"));
    assert_eq!(
        result.error.as_deref(),
        Some("The service encountered an unexpected internal error.")
    );
    let value = serde_json::to_value(&result).expect("serialize result");
    assert_exact_media_result_hides_internal_job_fields(&value);
}

#[test]
fn exact_media_generation_result_returns_failed_video_diagnostic_object_for_current_adapters() {
    let cases = [
        (
            "worldrouter",
            "worldrouter_video",
            "seedance-2.0-fast",
            "wr-task-1",
            "The service encountered an unexpected internal error.",
        ),
        (
            "byteplus",
            "byteplus_video",
            "dreamina-seedance-2-0-fast-260128",
            "bp-task-1",
            "The request failed because the output video may be related to copyright restrictions.",
        ),
        (
            "relaydance",
            "relaydance_video",
            "doubao-seedance-2-0-720p",
            "rd-task-1",
            "content blocked upstream",
        ),
    ];

    for (provider, adapter, model, task_id, message) in cases {
        let mut job = MediaJob::new(
            format!("job-{provider}"),
            MediaKind::Video,
            provider,
            model,
            "animate a robot",
            1,
            1,
        );
        job.adapter = Some(adapter.to_string());
        job.provider_job_id = Some(task_id.to_string());
        job.remote_status = Some("failed".to_string());
        job.error = Some(message.to_string());
        job.transition(MediaJobStatus::Failed, 2).unwrap();

        let result = exact_media_generation_result(job, Vec::new());
        let diagnostic = result.diagnostic.expect("diagnostic");

        assert_eq!(diagnostic.kind, "video");
        assert_eq!(diagnostic.provider_id, provider);
        assert_eq!(diagnostic.adapter.as_deref(), Some(adapter));
        assert_eq!(diagnostic.model_id.as_deref(), Some(model));
        assert_eq!(diagnostic.phase.as_deref(), Some("poll"));
        assert_eq!(diagnostic.provider_job_id.as_deref(), Some(task_id));
        assert_eq!(diagnostic.remote_status.as_deref(), Some("failed"));
        assert!(diagnostic.error.contains(message));
    }
}

#[test]
fn generate_exact_image_dispatches_to_minimax_adapter() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let request_text = read_http_request(&mut stream);
        let body = json!({
            "data": {"image_base64": ["aW1hZ2UtYnl0ZXM="]},
            "base_resp": {"status_code": 0, "status_msg": "success"}
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
    let registry = minimax_registry(format!("http://{address}"));
    let workspace = tempdir().expect("tempdir");

    let result = generate_exact_image_with_cache(
        &registry,
        &auth_store(),
        workspace.path(),
        ExactImageGenerationRequest {
            provider_id: "minimax".to_string(),
            model_id: "image-01".to_string(),
            prompt: "draw a precise icon".to_string(),
            parameters: BTreeMap::from([
                ("aspect_ratio".to_string(), "16:9".to_string()),
                ("response_format".to_string(), "base64".to_string()),
            ]),
            count: Some(1),
        },
        &ExactMediaDiscoveryCache::empty(),
    )
    .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.starts_with("POST /v1/image_generation HTTP/1.1"));
    assert!(request_text.contains("\"aspect_ratio\":\"16:9\""));
    assert_eq!(result.provider_id, "minimax");
    assert_eq!(result.model_id, "image-01");
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"image-bytes"
    );
}

#[test]
fn generate_exact_image_with_cache_executes_discovered_chat_image_model() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let request_text = read_http_request(&mut stream);
        let body = json!({
            "choices": [{
                "message": {
                    "images": [{"b64_json": "aW1hZ2UtYnl0ZXM="}]
                }
            }]
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
    let registry = chat_router_registry(format!("http://{address}"));
    let workspace = tempdir().expect("tempdir");
    let cache = discovered_chat_image_cache();

    let result = generate_exact_image_with_cache(
        &registry,
        &auth_store_for("openrouter"),
        workspace.path(),
        ExactImageGenerationRequest {
            provider_id: "openrouter".to_string(),
            model_id: "openrouter/image-chat".to_string(),
            prompt: "draw a precise icon".to_string(),
            parameters: BTreeMap::new(),
            count: Some(1),
        },
        &cache,
    )
    .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.starts_with("POST /chat/completions HTTP/1.1"));
    assert!(request_text.contains("\"model\":\"openrouter/image-chat\""));
    assert_eq!(result.provider_id, "openrouter");
    assert_eq!(result.model_id, "openrouter/image-chat");
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"image-bytes"
    );
}

#[test]
fn generate_exact_image_prunes_stale_undeclared_parameters_before_http() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
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
    let registry = byteplus_seedream_registry(format!("http://{address}"));
    let workspace = tempdir().expect("tempdir");

    generate_exact_image_with_cache(
        &registry,
        &auth_store_for("byteplus"),
        workspace.path(),
        ExactImageGenerationRequest {
            provider_id: "byteplus".to_string(),
            model_id: "seedream-4-5-251128".to_string(),
            prompt: "draw a precise icon".to_string(),
            parameters: BTreeMap::from([
                ("mode".to_string(), "2K HD".to_string()),
                ("ratio".to_string(), "Auto".to_string()),
                ("output".to_string(), "1".to_string()),
                ("output_format".to_string(), "png".to_string()),
            ]),
            count: None,
        },
        &ExactMediaDiscoveryCache::empty(),
    )
    .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.contains("\"size\":\"2K\""));
    assert!(request_text.contains("\"response_format\":\"b64_json\""));
    assert!(!request_text.contains("output_format"));
}

#[test]
fn generate_exact_image_with_cache_rejects_discovered_model_missing_from_cache_before_http() {
    let registry = chat_router_registry("http://127.0.0.1:9".to_string());
    let workspace = tempdir().expect("tempdir");

    let error = generate_exact_image_with_cache(
        &registry,
        &auth_store_for("openrouter"),
        workspace.path(),
        ExactImageGenerationRequest {
            provider_id: "openrouter".to_string(),
            model_id: "openrouter/image-chat".to_string(),
            prompt: "draw a precise icon".to_string(),
            parameters: BTreeMap::new(),
            count: Some(1),
        },
        &ExactMediaDiscoveryCache::empty(),
    )
    .expect_err("missing discovery cache should fail");

    assert!(error.to_string().contains("unknown media model"), "{error}");
}

#[test]
fn list_video_capabilities_exposes_multiple_static_seedance_models() {
    let mut registry = ProviderRegistry::new();
    registry.register_many(vec![
        bundled_provider(
            "byteplus",
            include_str!("../../../resources/providers/byteplus.yaml"),
        ),
        bundled_provider(
            "relaydance",
            include_str!("../../../resources/providers/relaydance.yaml"),
        ),
    ]);
    let mut auth = AuthStore::default();
    auth.set_api_key("byteplus", "sk-test");
    auth.set_api_key("relaydance", "sk-test");

    let capabilities = list_exact_media_capabilities_with_cache(
        &registry,
        &auth,
        Some("video"),
        &ExactMediaDiscoveryCache::empty(),
    );
    let ids = capabilities
        .iter()
        .map(|capability| capability.model_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        ids,
        std::collections::BTreeSet::from([
            "dreamina-seedance-2-0",
            "dreamina-seedance-2-0-fast",
            "doubao-seedance-2-0",
            "doubao-seedance-2-0-fast",
            "seedance-1-5-pro",
            "seedance-nsfw",
            "seedance-fast-nsfw",
            "grok-imagine-video",
            "grok-imagine-video-1.5-preview",
            "happyhorse-1.0-t2v",
        ])
    );
    assert!(capabilities.iter().all(|capability| {
        capability.kind == "video"
            && capability.operation == "generate"
            && capability.status == "available"
            && capability.source == "static"
    }));

    // seedance-1-5-pro folds the with/without-audio upstream models behind a
    // single bool selector axis.
    let pro = capabilities
        .iter()
        .find(|capability| capability.model_id == "seedance-1-5-pro")
        .expect("seedance 1.5 pro logical model");
    let audio = pro
        .axes
        .iter()
        .find(|axis| axis.id == "audio")
        .expect("audio axis");
    assert_eq!(audio.role, AxisRole::Selector);

    // byteplus exposes duration as a numeric range param axis.
    let byteplus = capabilities
        .iter()
        .find(|capability| capability.model_id == "dreamina-seedance-2-0")
        .expect("byteplus logical model");
    let duration = byteplus
        .axes
        .iter()
        .find(|axis| axis.id == "duration")
        .expect("duration axis");
    assert_eq!(duration.request_field.as_deref(), Some("duration"));
    assert_eq!(duration.wire_type, WireType::Number);

    // grok stays prompt-only (no axes).
    let grok_video = capabilities
        .iter()
        .find(|capability| capability.model_id == "grok-imagine-video")
        .expect("grok imagine video model");
    assert!(grok_video.axes.is_empty());
}

#[test]
fn list_video_capabilities_exposes_current_worldrouter_seedance_settings() {
    let mut registry = ProviderRegistry::new();
    registry.register(bundled_provider(
        "worldrouter",
        include_str!("../../../resources/providers/worldrouter.yaml"),
    ));

    let capabilities = list_exact_media_capabilities_with_cache(
        &registry,
        &auth_store_for("worldrouter"),
        Some("video"),
        &ExactMediaDiscoveryCache::empty(),
    );
    let ids = capabilities
        .iter()
        .map(|capability| capability.model_id.as_str())
        .collect::<std::collections::BTreeSet<_>>();

    assert_eq!(
        ids,
        std::collections::BTreeSet::from(["seedance-2.0", "seedance-2.0-fast"])
    );
    assert!(capabilities.iter().all(|capability| {
        capability.provider_id == "worldrouter"
            && capability.kind == "video"
            && capability.operation == "generate"
            && capability.status == "available"
    }));
    assert!(capabilities
        .iter()
        .all(|capability| capability.adapter == "worldrouter_video"));

    let seedance = capabilities
        .iter()
        .find(|capability| capability.model_id == "seedance-2.0")
        .expect("seedance-2.0 capability");
    let axis_ids = seedance
        .axes
        .iter()
        .map(|axis| axis.id.as_str())
        .collect::<Vec<_>>();
    assert_eq!(axis_ids, vec!["resolution", "duration"]);

    let resolution = seedance
        .axes
        .iter()
        .find(|axis| axis.id == "resolution")
        .expect("resolution axis");
    assert_eq!(resolution.label, "Mode");
    assert_eq!(resolution.role, AxisRole::Selector);
    assert!(
        matches!(&resolution.control, ControlKind::Enum { values, default } if default == "1080p" && values == &vec!["720p".to_string(), "1080p".to_string()])
    );

    let duration = seedance
        .axes
        .iter()
        .find(|axis| axis.id == "duration")
        .expect("duration axis");
    assert_eq!(duration.label, "Duration");
    assert_eq!(duration.request_field.as_deref(), Some("duration"));
    assert_eq!(duration.wire_type, WireType::Number);
}

#[test]
fn exact_media_generation_rejects_unsupported_video_parameter() {
    let (registry, auth, cache, workspace) = replicate_video_runtime_fixture();
    let request = ExactMediaGenerationRequest {
        kind: "video".to_string(),
        provider_id: "replicate".to_string(),
        model_id: "owner/model-version".to_string(),
        operation: "generate".to_string(),
        prompt: "animate a logo".to_string(),
        image_references: Vec::new(),
        parameters: BTreeMap::from([
            ("aspect_ratio".to_string(), "1:1".to_string()),
            ("duration_seconds".to_string(), "5".to_string()),
        ]),
        count: Some(1),
    };

    let error =
        generate_exact_media_with_cache(&registry, &auth, workspace.path(), request, &cache)
            .unwrap_err()
            .to_string();
    // Value validation now lives in the resolver's axis checks.
    assert!(error.contains("aspect_ratio"), "{error}");
}

#[test]
fn exact_media_generation_rejects_unknown_video_model_before_http() {
    let (registry, auth, cache, workspace) = replicate_video_runtime_fixture();
    let request = ExactMediaGenerationRequest {
        kind: "video".to_string(),
        provider_id: "replicate".to_string(),
        model_id: "owner/unknown-model".to_string(),
        operation: "generate".to_string(),
        prompt: "animate a logo".to_string(),
        image_references: Vec::new(),
        parameters: BTreeMap::from([
            ("aspect_ratio".to_string(), "16:9".to_string()),
            ("duration_seconds".to_string(), "5".to_string()),
        ]),
        count: Some(1),
    };

    let error =
        generate_exact_media_with_cache(&registry, &auth, workspace.path(), request, &cache)
            .unwrap_err()
            .to_string();
    assert!(error.contains("unknown media model"), "{error}");
}

#[test]
fn exact_media_discovery_cache_uses_ttl_boundary() {
    let cache = ExactMediaDiscoveryCache::from_inner_for_test(
        crate::media::resolver::MediaDiscoveryCache::default(),
        1_000,
    );

    assert!(cache.is_fresh_at(1_000 + MEDIA_DISCOVERY_TTL_MS - 1));
    assert!(!cache.is_fresh_at(1_000 + MEDIA_DISCOVERY_TTL_MS + 1));
}
