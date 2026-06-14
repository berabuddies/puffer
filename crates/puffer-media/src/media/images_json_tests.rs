use super::*;
use crate::media::MediaGenerationService;
use indexmap::IndexMap;
use puffer_provider_registry::{
    AuthMode, AuthStore, Axis, AxisRole, ControlKind, MediaBatchDescriptor, MediaBatchMode,
    MediaExecutionDescriptor, MediaExecutionKind, MediaKindDescriptor, MediaModelDescriptor,
    MediaOperation, ModelDescriptor, ProviderDescriptor, ProviderMediaDescriptor, ProviderRegistry,
    Variant, Variants, WireType,
};
use serde_json::json;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::thread;
use tempfile::tempdir;

fn registry_with_provider(base_url: String) -> ProviderRegistry {
    registry_with_provider_id("exact-provider", base_url)
}

fn registry_with_provider_id(provider_id: &str, base_url: String) -> ProviderRegistry {
    registry_with_provider_parameters(provider_id, base_url, image_parameters())
}

fn registry_with_provider_parameters(
    provider_id: &str,
    base_url: String,
    parameters: Vec<Axis>,
) -> ProviderRegistry {
    registry_with_provider_parameters_and_batch(
        provider_id,
        base_url,
        parameters,
        per_image_batch(),
    )
}

fn registry_with_provider_parameters_and_batch(
    provider_id: &str,
    base_url: String,
    parameters: Vec<Axis>,
    batch: MediaBatchDescriptor,
) -> ProviderRegistry {
    let mut registry = ProviderRegistry::new();
    registry.register(ProviderDescriptor {
        id: provider_id.to_string(),
        display_name: "Exact Provider".to_string(),
        base_url,
        default_api: "openai-responses".to_string(),
        auth_modes: vec![AuthMode::ApiKey],
        headers: IndexMap::from([("x-provider-header".to_string(), "present".to_string())]),
        query_params: IndexMap::from([("api-version".to_string(), "2026-06-05".to_string())]),
        chat_completions_path: None,
        discovery: None,
        media: Some(ProviderMediaDescriptor {
            image: Some(MediaKindDescriptor {
                discovery: None,
                execution: Some(MediaExecutionDescriptor {
                    adapter: MediaExecutionKind::ImagesJson,
                    base_url: None,
                    path: "/custom/images".to_string(),
                    batch,
                    prompt_format: Default::default(),
                }),
                models: vec![MediaModelDescriptor {
                    id: "exact-image-model".to_string(),
                    display_name: Some("Exact Image Model".to_string()),
                    max_outputs: None,
                    execution: None,
                    operations: vec![MediaOperation::Generate],
                    axes: parameters,
                    media_map: None,
                    variants: Variants::Single(Variant {
                        model_id: "exact-image-model".to_string(),
                        base_params: ::std::collections::BTreeMap::new(),
                    }),
                }],
            }),
            video: None,
        }),
        models: Vec::<ModelDescriptor>::new(),
    });
    registry
}

fn param_axis(id: &str, label: &str, values: &[&str], default: &str, request_field: &str) -> Axis {
    Axis {
        id: id.to_string(),
        label: label.to_string(),
        role: AxisRole::Param,
        control: ControlKind::Enum {
            values: values.iter().map(|v| v.to_string()).collect(),
            default: default.to_string(),
        },
        request_field: Some(request_field.to_string()),
        wire_type: WireType::String,
    }
}

fn image_parameters() -> Vec<Axis> {
    vec![
        param_axis("size", "Size", &["1024x1024"], "1024x1024", "size"),
        param_axis("quality", "Quality", &["auto"], "auto", "quality"),
        param_axis(
            "output_format",
            "Output format",
            &["png", "webp"],
            "png",
            "output_format",
        ),
    ]
}

fn per_image_batch() -> MediaBatchDescriptor {
    MediaBatchDescriptor {
        mode: MediaBatchMode::PerImage,
        max_images_per_call: None,
    }
}

fn exact_batch(limit: u8) -> MediaBatchDescriptor {
    MediaBatchDescriptor {
        mode: MediaBatchMode::Exact,
        max_images_per_call: Some(limit),
    }
}

fn sequential_generation_parameter() -> Axis {
    param_axis(
        "sequential_image_generation",
        "Sequential image generation",
        &["disabled", "auto"],
        "disabled",
        "sequential_image_generation",
    )
}

fn auth_store() -> AuthStore {
    let mut auth = AuthStore::default();
    auth.set_api_key("exact-provider", "sk-secret");
    auth
}

fn codex_auth_store() -> AuthStore {
    let mut auth = AuthStore::default();
    auth.set_api_key("codex", "sk-codex-secret");
    auth
}

fn request() -> ImagesJsonGenerationRequest {
    ImagesJsonGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        adapter: "images_json".to_string(),
        prompt: "draw a precise icon".to_string(),
        parameters: BTreeMap::from([
            ("size".to_string(), "1024x1024".to_string()),
            ("quality".to_string(), "auto".to_string()),
            ("output_format".to_string(), "png".to_string()),
        ]),
        count: 1,
    }
}

fn read_http_request(stream: &mut std::net::TcpStream) -> String {
    let mut buffer = [0_u8; 8192];
    let size = stream.read(&mut buffer).expect("read request");
    String::from_utf8_lossy(&buffer[..size]).to_string()
}

fn spawn_image_server_with_body(body: &'static str) -> (String, std::thread::JoinHandle<String>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        let mut buffer = [0_u8; 4096];
        let n = stream.read(&mut buffer).unwrap();
        let request = String::from_utf8_lossy(&buffer[..n]).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).unwrap();
        request
    });
    (format!("http://{addr}"), handle)
}

fn spawn_repeated_image_server_with_body(
    body: &'static str,
    expected_requests: usize,
) -> (String, std::thread::JoinHandle<Vec<String>>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let mut requests = Vec::new();
        for _ in 0..expected_requests {
            let (mut stream, _) = listener.accept().unwrap();
            let mut buffer = [0_u8; 4096];
            let n = stream.read(&mut buffer).unwrap();
            requests.push(String::from_utf8_lossy(&buffer[..n]).to_string());
            let response = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            stream.write_all(response.as_bytes()).unwrap();
        }
        requests
    });
    (format!("http://{addr}"), handle)
}

fn load_single_saved_job(root: &std::path::Path) -> crate::media::MediaJob {
    let jobs_dir = root.join(".puffer").join("media").join("jobs");
    let paths = std::fs::read_dir(&jobs_dir)
        .expect("jobs dir")
        .map(|entry| entry.expect("job dir entry").path())
        .collect::<Vec<_>>();
    assert_eq!(paths.len(), 1);
    serde_json::from_slice(&std::fs::read(&paths[0]).expect("job sidecar")).expect("job json")
}

#[test]
fn images_json_repeats_single_image_calls_in_per_image_mode() {
    let (base_url, server) =
        spawn_repeated_image_server_with_body(r#"{"data":[{"b64_json":"aW1hZ2U="}]}"#, 2);
    let mut parameters = image_parameters();
    parameters.push(sequential_generation_parameter());
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        parameters,
        per_image_batch(),
    );
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("exact-provider", "sk-test");
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let request = ImagesJsonGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        adapter: "images_json".to_string(),
        prompt: "draw two images".to_string(),
        parameters: BTreeMap::from([
            ("size".to_string(), "1024x1024".to_string()),
            ("quality".to_string(), "auto".to_string()),
            ("output_format".to_string(), "png".to_string()),
            (
                "sequential_image_generation".to_string(),
                "disabled".to_string(),
            ),
        ]),
        count: 2,
    };

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store, &service, request)
        .unwrap();

    let requests = server.join().unwrap();
    assert_eq!(requests.len(), 2);
    assert!(requests.iter().all(|request| !request.contains("\"n\"")));
    assert!(requests
        .iter()
        .all(|request| request.contains("\"sequential_image_generation\":\"disabled\"")));
    assert_eq!(result.job.requested_count, 2);
    assert_eq!(result.job.artifact_ids.len(), 2);
    assert_eq!(result.artifacts.len(), 2);
    assert_eq!(result.artifacts[0].metadata["index"], 0);
    assert_eq!(result.artifacts[1].metadata["index"], 1);
}

#[test]
fn images_json_uses_exact_batch_mode_when_descriptor_opts_in() {
    let (base_url, server) = spawn_image_server_with_body(
        r#"{"data":[{"b64_json":"aW1hZ2UtMQ=="},{"b64_json":"aW1hZ2UtMg=="}]}"#,
    );
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        image_parameters(),
        exact_batch(4),
    );
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let mut request = request();
    request.count = 2;

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store(), &service, request)
        .unwrap();

    let request_text = server.join().unwrap();
    assert!(request_text.contains("\"n\":2"));
    assert_eq!(result.artifacts.len(), 2);
}

#[test]
fn images_json_failed_later_per_image_call_preserves_first_artifact() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let mut requests = Vec::new();
        for index in 0..2 {
            let (mut stream, _) = listener.accept().expect("request");
            requests.push(read_http_request(&mut stream));
            if index == 0 {
                let body = r#"{"data":[{"b64_json":"aW1hZ2U="}]}"#;
                let response = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).expect("response");
            } else {
                let body = "rate limited";
                let response = format!(
                    "HTTP/1.1 500 Internal Server Error\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
                    body.len(),
                    body
                );
                stream.write_all(response.as_bytes()).expect("response");
            }
        }
        requests
    });
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        format!("http://{address}"),
        image_parameters(),
        per_image_batch(),
    );
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let mut request = request();
    request.count = 2;

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store(), &service, request)
        .expect("partial generation succeeds");

    assert_eq!(server.join().unwrap().len(), 2);
    assert_eq!(result.job.requested_count, 2);
    assert_eq!(result.job.status, crate::media::MediaJobStatus::Succeeded);
    assert_eq!(result.job.produced_count(), 1);
    assert_eq!(result.artifacts.len(), 1);
    assert_eq!(std::fs::read(&result.artifacts[0].path).unwrap(), b"image");
    assert!(service_dir.path().join(".puffer/media/images").exists());
}

#[test]
fn images_json_persists_multiple_response_images_under_one_job() {
    let (base_url, server) = spawn_image_server_with_body(
        r#"{"data":[{"b64_json":"aW1hZ2UtMQ=="},{"b64_json":"aW1hZ2UtMg=="}]}"#,
    );
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        image_parameters(),
        exact_batch(4),
    );
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("exact-provider", "sk-test");
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let request = ImagesJsonGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        adapter: "images_json".to_string(),
        prompt: "draw two images".to_string(),
        parameters: BTreeMap::from([
            ("size".to_string(), "1024x1024".to_string()),
            ("quality".to_string(), "auto".to_string()),
            ("output_format".to_string(), "png".to_string()),
        ]),
        count: 2,
    };

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store, &service, request)
        .unwrap();

    let request_text = server.join().unwrap();
    assert!(request_text.contains("\"n\":2"));
    assert_eq!(result.job.requested_count, 2);
    assert_eq!(result.job.artifact_ids.len(), 2);
    assert_eq!(result.artifacts.len(), 2);
    assert_ne!(result.artifacts[0].id, result.artifacts[1].id);
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"image-1"
    );
    assert_eq!(
        std::fs::read(&result.artifacts[1].path).unwrap(),
        b"image-2"
    );
    assert_eq!(result.artifacts[0].metadata["index"], 0);
    assert_eq!(result.artifacts[1].metadata["index"], 1);
}

#[test]
fn images_json_exact_batch_underproduction_returns_partial_success() {
    let (base_url, server) =
        spawn_image_server_with_body(r#"{"data":[{"b64_json":"aW1hZ2UtMQ=="}]}"#);
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        image_parameters(),
        exact_batch(4),
    );
    let mut auth_store = AuthStore::default();
    auth_store.set_api_key("exact-provider", "sk-test");
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let request = ImagesJsonGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        adapter: "images_json".to_string(),
        prompt: "draw two images".to_string(),
        parameters: BTreeMap::from([
            ("size".to_string(), "1024x1024".to_string()),
            ("quality".to_string(), "auto".to_string()),
            ("output_format".to_string(), "png".to_string()),
        ]),
        count: 2,
    };

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store, &service, request)
        .expect("under-produced exact batch is partial success");

    let request_text = server.join().unwrap();
    let saved_job = load_single_saved_job(service_dir.path());
    assert!(request_text.contains("\"n\":2"));
    assert_eq!(saved_job.status, crate::media::MediaJobStatus::Succeeded);
    assert_eq!(saved_job.requested_count, 2);
    assert_eq!(saved_job.artifact_ids.len(), 1);
    assert_eq!(result.artifacts.len(), 1);
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"image-1"
    );
}

#[test]
fn request_body_uses_selected_model_and_descriptor_path() {
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
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");

    let result = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request(),
        )
        .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.starts_with("POST /custom/images?api-version=2026-06-05 HTTP/1.1"));
    assert!(request_text.contains("authorization: Bearer sk-secret"));
    assert!(request_text.contains("x-provider-header: present"));
    assert!(request_text.contains("\"model\":\"exact-image-model\""));
    assert!(request_text.contains("\"size\":\"1024x1024\""));
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"image-bytes"
    );
    assert_eq!(result.artifacts[0].metadata["adapter"], "images_json");
}

#[test]
fn request_body_uses_descriptor_request_field_mapping() {
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
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");
    let mut request = request();
    // Adapter receives parameters already keyed by upstream request field.
    request.parameters.remove("size");
    request
        .parameters
        .insert("resolution".to_string(), "2k".to_string());

    ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request,
        )
        .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.contains("\"resolution\":\"2k\""));
}

#[test]
fn request_body_preserves_sequential_generation_descriptor_default() {
    let (base_url, server) = spawn_image_server_with_body(
        r#"{"data":[{"b64_json":"aW1hZ2UtMQ=="},{"b64_json":"aW1hZ2UtMg=="}]}"#,
    );
    let mut parameters = image_parameters();
    parameters.push(sequential_generation_parameter());
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        parameters,
        exact_batch(4),
    );
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let mut request = ImagesJsonGenerationRequest {
        count: 2,
        ..request()
    };
    request.prompt = "draw two images".to_string();
    request.parameters.insert(
        "sequential_image_generation".to_string(),
        "disabled".to_string(),
    );

    let result = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store(), &service, request)
        .unwrap();

    let request_text = server.join().unwrap();
    assert!(request_text.contains("\"n\":2"));
    assert!(request_text.contains("\"sequential_image_generation\":\"disabled\""));
    assert_eq!(
        result.artifacts[0].metadata["parameters"]["sequential_image_generation"],
        "disabled"
    );
}

#[test]
fn images_json_zero_outputs_fails_without_artifacts() {
    let (base_url, server) = spawn_image_server_with_body(r#"{"data":[]}"#);
    let registry = registry_with_provider_parameters_and_batch(
        "exact-provider",
        base_url,
        image_parameters(),
        exact_batch(4),
    );
    let service_dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(service_dir.path());
    let mut request = request();
    request.count = 2;

    let error = ImagesJsonAdapter::new()
        .unwrap()
        .execute(&registry, &auth_store(), &service, request)
        .expect_err("zero outputs fail");

    let _request_text = server.join().unwrap();
    let saved_job = load_single_saved_job(service_dir.path());
    assert_eq!(error.to_string(), "image generation produced no images");
    assert_eq!(saved_job.status, crate::media::MediaJobStatus::Failed);
    assert!(saved_job.artifact_ids.is_empty());
    assert!(!service_dir.path().join(".puffer/media/images").exists());
}

#[test]
fn artifact_metadata_uses_resolved_descriptor_defaults() {
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
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");
    let mut request = request();
    request
        .parameters
        .insert("output_format".to_string(), "jpeg".to_string());

    let result = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request,
        )
        .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.contains("\"output_format\":\"jpeg\""));
    assert_eq!(result.artifacts[0].mime_type, "image/jpeg");
    assert_eq!(
        result.artifacts[0]
            .path
            .extension()
            .and_then(|value| value.to_str()),
        Some("jpeg")
    );
    assert_eq!(
        result.artifacts[0].metadata["parameters"]["output_format"],
        "jpeg"
    );
    assert_eq!(result.artifacts[0].metadata["mimeType"], "image/jpeg");
}

#[test]
fn artifact_format_follows_response_bytes_when_output_format_undeclared() {
    let jpeg_b64 = BASE64_STANDARD.encode([0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10]);
    let response_b64 = jpeg_b64.clone();
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        read_http_request(&mut stream);
        let body = json!({ "data": [{"b64_json": response_b64}] }).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).expect("response");
    });
    // Model declares no output_format parameter (mirrors BytePlus Seedream 4.5/4.0).
    let parameters = vec![param_axis("size", "Size", &["2K"], "2K", "size")];
    let registry = registry_with_provider_parameters(
        "exact-provider",
        format!("http://{address}"),
        parameters,
    );
    let service_dir = tempdir().expect("tempdir");
    let request = ImagesJsonGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        adapter: "images_json".to_string(),
        prompt: "draw a precise icon".to_string(),
        parameters: BTreeMap::from([("size".to_string(), "2K".to_string())]),
        count: 1,
    };

    let result = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request,
        )
        .expect("generation succeeds");

    server.join().expect("server");
    assert_eq!(result.artifacts[0].mime_type, "image/jpeg");
    assert_eq!(
        result.artifacts[0]
            .path
            .extension()
            .and_then(|value| value.to_str()),
        Some("jpeg")
    );
    assert_eq!(result.artifacts[0].metadata["mimeType"], "image/jpeg");
}

#[test]
fn url_response_is_downloaded_before_success() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("generation request");
        let generation_request = read_http_request(&mut stream);
        let body = json!({
            "data": [{
                "url": format!("http://{address}/generated.png"),
                "revised_prompt": "draw a more precise icon"
            }]
        })
        .to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream
            .write_all(response.as_bytes())
            .expect("generation response");

        let (mut stream, _) = listener.accept().expect("download request");
        let download_request = read_http_request(&mut stream);
        let response = "HTTP/1.1 200 OK\r\ncontent-type: image/png\r\ncontent-length: 12\r\nconnection: close\r\n\r\ndownloaded!!";
        stream
            .write_all(response.as_bytes())
            .expect("download response");
        (generation_request, download_request)
    });
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");

    let result = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request(),
        )
        .expect("generation succeeds");

    let (_, download_request) = server.join().expect("server");
    assert!(download_request.starts_with("GET /generated.png HTTP/1.1"));
    assert_eq!(
        std::fs::read(&result.artifacts[0].path).unwrap(),
        b"downloaded!!"
    );
    assert_eq!(result.job.status, crate::media::MediaJobStatus::Succeeded);
    assert_eq!(
        result.artifacts[0].metadata["revisedPrompt"],
        "draw a more precise icon"
    );
    assert_eq!(
        result.artifacts[0].metadata["remoteSourceUrl"],
        format!("http://{address}/generated.png")
    );
}

#[test]
fn missing_image_data_returns_stable_error() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let body = json!({"data": [{}]}).to_string();
        let response = format!(
            "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).expect("response");
    });
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");

    let error = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request(),
        )
        .expect_err("missing image data should fail");

    server.join().expect("server");
    assert_eq!(
        error.to_string(),
        "image generation response did not contain an image"
    );
}

#[test]
fn external_http_image_url_is_rejected_before_download() {
    let value = json!({
        "data": [{"url": "http://example.com/generated.png"}]
    });

    let error = image_outputs_from_response(&Client::new(), &value, 1)
        .expect_err("external http URL should fail before download");

    assert_eq!(
        error.to_string(),
        "unsupported image response URL scheme `http`"
    );
}

#[test]
fn unsupported_request_field_in_parameters_fails_before_http_request() {
    // Value validation now lives in the resolver; the adapter only rejects
    // request fields outside its allowlist.
    let registry = registry_with_provider("http://127.0.0.1:9".to_string());
    let service_dir = tempdir().expect("tempdir");
    let mut request = request();
    request
        .parameters
        .insert("watermark".to_string(), "on".to_string());

    let error = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request,
        )
        .expect_err("unsupported request field should fail");

    assert_eq!(
        error.to_string(),
        "image generation request field unsupported: watermark"
    );
}

#[test]
fn resolver_rejects_unsupported_image_value() {
    // The resolver validates axis values for a logical image selection.
    let registry = registry_with_provider("http://127.0.0.1:9".to_string());
    let selection = crate::runtime::ExactImageGenerationRequest {
        provider_id: "exact-provider".to_string(),
        model_id: "exact-image-model".to_string(),
        prompt: "draw a precise icon".to_string(),
        parameters: BTreeMap::from([("size".to_string(), "2048x2048".to_string())]),
        count: Some(1),
    };

    let error = crate::runtime::resolved_exact_image_parameters_with_cache(
        &registry,
        &auth_store(),
        &selection,
        &crate::runtime::ExactMediaDiscoveryCache::empty(),
    )
    .expect_err("unsupported size value should fail");

    assert!(error.to_string().contains("size"), "{error}");
}

#[test]
fn provider_secrets_are_redacted_from_error_responses() {
    let listener = TcpListener::bind("127.0.0.1:0").expect("listener");
    let address = listener.local_addr().expect("address");
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().expect("request");
        let _request_text = read_http_request(&mut stream);
        let body = "bad sk-secret 2026-06-05";
        let response = format!(
            "HTTP/1.1 400 Bad Request\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{}",
            body.len(),
            body
        );
        stream.write_all(response.as_bytes()).expect("response");
    });
    let registry = registry_with_provider(format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");

    let error = ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request(),
        )
        .expect_err("provider error should fail");

    server.join().expect("server");
    let message = error.to_string();
    assert!(message.contains("[redacted]"), "{message}");
    assert!(!message.contains("sk-secret"), "{message}");
    assert!(!message.contains("2026-06-05"), "{message}");
}

#[test]
fn openai_provider_uses_codex_credentials_for_generation() {
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
    let registry = registry_with_provider_id("openai", format!("http://{address}"));
    let service_dir = tempdir().expect("tempdir");
    let request = ImagesJsonGenerationRequest {
        provider_id: "openai".to_string(),
        model_id: "exact-image-model".to_string(),
        ..request()
    };

    ImagesJsonAdapter::new()
        .expect("adapter")
        .execute(
            &registry,
            &codex_auth_store(),
            &MediaGenerationService::new(service_dir.path()),
            request,
        )
        .expect("generation succeeds");

    let request_text = server.join().expect("server");
    assert!(request_text.contains("authorization: Bearer sk-codex-secret"));
}
