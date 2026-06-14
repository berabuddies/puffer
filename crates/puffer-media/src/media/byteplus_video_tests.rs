use super::tests_support::{self, ScriptedBytes, ScriptedJson, ScriptedTransport};
use super::*;
use crate::{media_failure_diagnostic, ProviderHttpError};
use serde_json::json;
use std::cell::RefCell;
use std::collections::BTreeMap;

fn submit_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/byteplus_submit_task.json")).expect("fixture")
}

fn poll_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/byteplus_poll_task.json")).expect("fixture")
}

#[test]
fn parses_byteplus_submit_fixture() {
    let task = BytePlusVideoTask::from_submit_value(submit_fixture()).expect("task");
    assert!(!task.id.trim().is_empty());
}

#[test]
fn parses_byteplus_poll_fixture() {
    let task = BytePlusVideoTask::from_poll_value(poll_fixture()).expect("task");
    assert!(!task.status.trim().is_empty());
}

#[test]
fn byteplus_request_body_contains_model_and_prompt() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a cat".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };
    let body = request.request_body();
    assert_eq!(body["model"], json!("dreamina-seedance-2-0-fast-260128"));
    assert_eq!(body["content"][0]["type"], json!("text"));
    assert_eq!(body["content"][0]["text"], json!("a cat"));
}

#[test]
fn byteplus_request_body_includes_public_image_references() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "animate image 1".to_string(),
        image_references: vec!["https://example.com/person.png".to_string()],
        params: vec![],
    };
    let body = request.request_body();

    assert_eq!(body["content"][0]["type"], json!("text"));
    assert_eq!(body["content"][1]["type"], json!("image_url"));
    assert_eq!(
        body["content"][1]["image_url"]["url"],
        json!("https://example.com/person.png")
    );
    assert!(body["content"][1].get("role").is_none());
}

#[test]
fn byteplus_request_body_includes_asset_image_references() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "animate image 1".to_string(),
        image_references: vec!["asset://approved-person".to_string()],
        params: vec![],
    };
    let body = request.request_body();

    assert_eq!(
        body["content"][1]["image_url"]["url"],
        json!("asset://approved-person")
    );
}

#[test]
fn byteplus_request_body_encodes_duration_as_number() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a cat".to_string(),
        image_references: Vec::new(),
        params: vec![
            ("duration".to_string(), json!(5)),
            ("ratio".to_string(), json!("16:9")),
        ],
    };
    let body = request.request_body();

    assert_eq!(body["duration"], json!(5));
    assert_eq!(body["ratio"], json!("16:9"));
}

#[test]
fn byteplus_request_body_defaults_to_silent_video() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a cat".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };
    let body = request.request_body();

    // Silent by default; BytePlus audio moderation otherwise fails the job.
    assert_eq!(body["generate_audio"], json!(false));
}

#[test]
fn byteplus_request_body_allows_generate_audio_override() {
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a cat".to_string(),
        image_references: Vec::new(),
        params: vec![("generate_audio".to_string(), json!(true))],
    };
    let body = request.request_body();

    assert_eq!(body["generate_audio"], json!(true));
}

#[test]
fn byteplus_request_body_encodes_integer_fields_as_numbers() {
    let request = byteplus_video_request_from_parameters(
        "dreamina-seedance-2-0-260128".to_string(),
        "animate a calm lake".to_string(),
        Vec::new(),
        &BTreeMap::from([
            ("duration".to_string(), "5".to_string()),
            ("ratio".to_string(), "16:9".to_string()),
            ("resolution".to_string(), "720p".to_string()),
        ]),
    )
    .expect("request");

    let body = request.request_body();

    assert_eq!(body["duration"], json!(5));
    assert_eq!(body["ratio"], json!("16:9"));
    assert_eq!(body["resolution"], json!("720p"));
}

#[test]
fn byteplus_submit_http_failure_returns_media_diagnostic() {
    let service = MediaGenerationService::new(tempfile::tempdir().unwrap().path());
    let adapter = BytePlusVideoAdapter::with_transport(
        "token",
        "https://ark.ap-southeast.bytepluses.com/api/v3/contents/generations/tasks",
        "byteplus",
        tests_support::scripted_submit_error(ProviderHttpError::new(
            "submit video task",
            500,
            r#"{"error":{"code":"InternalError","message":"upstream internal error","request_id":"bp-req-1"}}"#,
        )),
    );
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a robot battle".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };

    let error = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect_err("submit should fail");
    let diagnostic = media_failure_diagnostic(&error).expect("diagnostic");

    assert_eq!(diagnostic.provider_id, "byteplus");
    assert_eq!(diagnostic.adapter.as_deref(), Some("byteplus_video"));
    assert_eq!(diagnostic.phase.as_deref(), Some("submit"));
    assert_eq!(diagnostic.http_status, Some(500));
    assert_eq!(diagnostic.provider_code.as_deref(), Some("InternalError"));
    assert_eq!(diagnostic.request_id.as_deref(), Some("bp-req-1"));
}

#[test]
fn poll_parser_failure_is_transient_and_keeps_polling() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = BytePlusVideoAdapter::with_transport(
        "token",
        "https://ark.ap-southeast.bytepluses.com/api/v3/contents/generations/tasks",
        "byteplus",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({ "id": "task-1" })),
            polls: RefCell::new(vec![json!({ "status": "running" })]),
            downloads: RefCell::new(Vec::new()),
        },
    );
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-260128".to_string(),
        prompt: "a cat".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };
    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");

    let polled = adapter
        .poll(&service, job.clone(), 2)
        .expect("poll returns Ok (transient)");

    assert_eq!(polled.status, MediaJobStatus::Queued);
    let saved = service.load_job(&job.id).expect("saved job");
    assert_eq!(saved.status, MediaJobStatus::Queued);
    let error = saved.error.as_deref().expect("poll error");
    assert!(error.contains("missing task id"), "{error}");
    assert!(error.contains("provider=byteplus"), "{error}");
    assert!(error.contains("adapter=byteplus_video"), "{error}");
    assert!(error.contains("phase=poll"), "{error}");
    assert!(saved.updated_at_ms >= saved.created_at_ms);
}

#[test]
fn byteplus_failed_poll_persists_remote_failure_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = BytePlusVideoAdapter::with_transport(
        "token",
        "https://ark.ap-southeast.bytepluses.com/api/v3/contents/generations/tasks",
        "byteplus",
        tests_support::scripted(
            json!({"id": "bp-task-1"}),
            vec![json!({
                "id": "bp-task-1",
                "status": "failed",
                "error": {"message": "content moderation rejected output"}
            })],
        ),
    );
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a robot battle".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };

    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let job = adapter.poll(&service, job, 2).expect("poll");
    let persisted = service.load_job(&job.id).expect("persisted job");

    assert_eq!(persisted.status, MediaJobStatus::Failed);
    assert_eq!(persisted.provider_job_id.as_deref(), Some("bp-task-1"));
    assert_eq!(persisted.remote_status.as_deref(), Some("failed"));
    assert_eq!(
        persisted.error.as_deref(),
        Some("content moderation rejected output")
    );
}

#[test]
fn byteplus_download_failure_returns_media_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = BytePlusVideoAdapter::with_transport(
        "token",
        "https://ark.ap-southeast.bytepluses.com/api/v3/contents/generations/tasks",
        "byteplus",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({"id": "bp-task-1"})),
            polls: RefCell::new(vec![json!({
                "id": "bp-task-1",
                "status": "succeeded",
                "content": {"video_url": "https://media.example.com/out.mp4"}
            })]),
            downloads: RefCell::new(vec![ScriptedBytes::Err("cdn returned 503".to_string())]),
        },
    );
    let request = BytePlusVideoRequest {
        model: "dreamina-seedance-2-0-fast-260128".to_string(),
        prompt: "a robot battle".to_string(),
        image_references: Vec::new(),
        params: vec![],
    };

    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let error = adapter
        .poll_until_terminal(
            &service,
            job,
            BytePlusVideoPollingConfig::default(),
            |_| {},
            || 2,
        )
        .expect_err("download should fail");
    let diagnostic = media_failure_diagnostic(&error).expect("diagnostic");

    assert_eq!(diagnostic.provider_id, "byteplus");
    assert_eq!(diagnostic.adapter.as_deref(), Some("byteplus_video"));
    assert_eq!(diagnostic.phase.as_deref(), Some("download"));
    assert_eq!(diagnostic.provider_job_id.as_deref(), Some("bp-task-1"));
}
