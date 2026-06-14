use super::tests_support::{self, ScriptedBytes, ScriptedJson, ScriptedTransport};
use super::*;
use crate::{media_failure_diagnostic, ProviderHttpError};
use std::cell::RefCell;
use std::time::Duration;

fn params(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

fn wire_types(pairs: &[(&str, WireType)]) -> BTreeMap<String, WireType> {
    pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
}

#[test]
fn splits_top_level_and_metadata_params() {
    let request = relaydance_video_request_from_parameters(
        "doubao-seedance-2-0-1080p".to_string(),
        "a cat".to_string(),
        &params(&[
            ("seconds", "5"),
            ("metadata.resolution", "1080p"),
            ("metadata.ratio", "16:9"),
        ]),
        &wire_types(&[("seconds", WireType::Number)]),
        Default::default(),
    )
    .expect("request");

    let body = request.request_body();
    assert_eq!(body["model"], json!("doubao-seedance-2-0-1080p"));
    assert_eq!(body["prompt"], json!("a cat"));
    assert_eq!(body["n"], json!(1));
    assert_eq!(body["seconds"], json!(5));
    assert_eq!(body["metadata"]["resolution"], json!("1080p"));
    assert_eq!(body["metadata"]["ratio"], json!("16:9"));
}

#[test]
fn omits_metadata_when_no_metadata_params() {
    let request = relaydance_video_request_from_parameters(
        "m".to_string(),
        "a cat".to_string(),
        &params(&[("seconds", "5")]),
        &BTreeMap::new(),
        Default::default(),
    )
    .expect("request");
    let body = request.request_body();
    assert!(body.get("metadata").is_none());
}

#[test]
fn builds_prompt_only_request_without_optional_params() {
    let request = relaydance_video_request_from_parameters(
        "grok-imagine-video".to_string(),
        "a city skyline".to_string(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        Default::default(),
    )
    .expect("request");

    let body = request.request_body();
    assert_eq!(body["model"], json!("grok-imagine-video"));
    assert_eq!(body["prompt"], json!("a city skyline"));
    assert_eq!(body["n"], json!(1));
    assert!(body.get("metadata").is_none());
    assert!(body.get("seconds").is_none());
}

#[test]
fn builds_content_array_request_for_worldrouter_format() {
    let request = relaydance_video_request_from_parameters(
        "seedance-2.0".to_string(),
        "a sunset".to_string(),
        &params(&[("resolution", "720p"), ("duration", "5")]),
        &wire_types(&[("duration", WireType::Number)]),
        VideoPromptFormat::ContentArray,
    )
    .expect("request");

    let body = request.request_body();
    assert_eq!(body["model"], json!("seedance-2.0"));
    assert_eq!(
        body["content"],
        json!([{"type": "text", "text": "a sunset"}])
    );
    assert!(body.get("prompt").is_none());
    assert!(body.get("n").is_none());
    assert_eq!(body["resolution"], json!("720p"));
    assert_eq!(body["duration"], json!(5));
}

#[test]
fn rejects_invalid_numeric_parameter_before_http() {
    let error = relaydance_video_request_from_parameters(
        "seedance-2.0".to_string(),
        "a sunset".to_string(),
        &params(&[("duration", "soon")]),
        &wire_types(&[("duration", WireType::Number)]),
        VideoPromptFormat::ContentArray,
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("duration"), "{error}");
}

#[test]
fn rejects_empty_prompt() {
    let error = relaydance_video_request_from_parameters(
        "m".to_string(),
        "   ".to_string(),
        &BTreeMap::new(),
        &BTreeMap::new(),
        Default::default(),
    )
    .unwrap_err()
    .to_string();
    assert!(error.contains("prompt is required"));
}

fn relaydance_poll_fixture() -> serde_json::Value {
    serde_json::from_str(include_str!("fixtures/relaydance_poll_task.json")).expect("fixture")
}

#[test]
fn parses_relaydance_poll_fixture() {
    let task = RelaydanceVideoTask::from_value(relaydance_poll_fixture(), "poll video task")
        .expect("task");
    assert!(!task.id.trim().is_empty());
    assert!(!task.status.trim().is_empty());
}

#[test]
fn relaydance_shape_summary_lists_top_level_keys() {
    let summary = relaydance_response_shape_summary(&relaydance_poll_fixture());
    assert!(summary.contains("keys=["));
}

#[test]
fn parses_relaydance_completed_task_with_task_id_and_url() {
    let task = RelaydanceVideoTask::from_value(
        json!({
            "task_id": "task-1",
            "status": "succeeded",
            "url": "https://example.com/video.mp4"
        }),
        "poll video task",
    )
    .expect("task");

    assert_eq!(task.id, "task-1");
    assert_eq!(task.status, "succeeded");
    assert_eq!(
        task.video_url.as_deref(),
        Some("https://example.com/video.mp4")
    );
}

#[test]
fn relaydance_missing_task_id_reports_phase_and_keys() {
    let error = RelaydanceVideoTask::from_value(
        json!({
            "code": "ok",
            "message": "accepted",
            "data": { "status": "running" }
        }),
        "poll video task",
    )
    .unwrap_err()
    .to_string();

    assert!(error.contains("poll video task response missing task id"));
    assert!(error.contains("keys=[code,data,message]"));
}

#[test]
fn parses_completed_task_with_metadata_url() {
    let value = json!({
        "id": "vid-1",
        "status": "completed",
        "metadata": { "url": "https://cdn.example.com/v.mp4" }
    });
    let task = RelaydanceVideoTask::from_value(value, "poll video task").expect("task");
    assert_eq!(task.id, "vid-1");
    assert_eq!(task.media_status(), MediaJobStatus::Succeeded);
    assert_eq!(
        task.video_url.as_deref(),
        Some("https://cdn.example.com/v.mp4")
    );
}

#[test]
fn parses_failed_task_error_message() {
    let value = json!({
        "id": "vid-2",
        "status": "failed",
        "error": { "code": "x", "message": "content blocked" }
    });
    let task = RelaydanceVideoTask::from_value(value, "poll video task").expect("task");
    assert_eq!(task.media_status(), MediaJobStatus::Failed);
    assert_eq!(task.error.as_deref(), Some("content blocked"));
}

#[test]
fn unknown_status_maps_to_non_terminal() {
    let value = json!({ "id": "v", "status": "weird" });
    let task = RelaydanceVideoTask::from_value(value, "poll video task").expect("task");
    assert_eq!(task.media_status(), MediaJobStatus::Running);
    assert!(!task.media_status().is_terminal());
}

#[test]
fn parses_failure_status_with_fail_reason() {
    let value = json!({
        "data": { "id": "v", "status": "FAILURE", "fail_reason": "content blocked upstream" }
    });
    let task = RelaydanceVideoTask::from_value(value, "poll video task").expect("task");
    assert_eq!(task.media_status(), MediaJobStatus::Failed);
    assert_eq!(task.error.as_deref(), Some("content blocked upstream"));
}

#[test]
fn relaydance_submit_http_failure_returns_media_diagnostic() {
    let service = MediaGenerationService::new(tempfile::tempdir().unwrap().path());
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://api.relaydance.local/v1/video/generations",
        "relaydance",
        tests_support::scripted_submit_error(ProviderHttpError::new(
            "submit video task",
            429,
            r#"{"error":{"code":"rate_limited","message":"too many tasks"}}"#,
        )),
    );
    let request = RelaydanceVideoRequest {
        model: "seedance-nsfw-720p".to_string(),
        prompt: "a robot battle".to_string(),
        params: Vec::new(),
        prompt_format: VideoPromptFormat::Prompt,
    };

    let error = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect_err("submit should fail");
    let diagnostic = media_failure_diagnostic(&error).expect("diagnostic");

    assert_eq!(diagnostic.provider_id, "relaydance");
    assert_eq!(diagnostic.adapter.as_deref(), Some("relaydance_video"));
    assert_eq!(diagnostic.phase.as_deref(), Some("submit"));
    assert_eq!(diagnostic.http_status, Some(429));
    assert_eq!(diagnostic.provider_code.as_deref(), Some("rate_limited"));
    assert!(diagnostic.hint.unwrap().contains("rate limit"));
}

#[test]
fn submit_then_poll_downloads_video_artifact() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let transport = ScriptedTransport {
        submit: ScriptedJson::Ok(json!({ "id": "vid-9", "status": "queued" })),
        polls: RefCell::new(vec![
            json!({ "id": "vid-9", "status": "in_progress" }),
            json!({ "id": "vid-9", "status": "completed", "metadata": { "url": "https://cdn.example.com/v.mp4" } }),
        ]),
        downloads: RefCell::new(vec![ScriptedBytes::Ok(b"MP4BYTES".to_vec())]),
    };
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        transport,
    );

    let request = RelaydanceVideoRequest {
        model: "m".into(),
        prompt: "a cat".into(),
        params: vec![],
        prompt_format: Default::default(),
    };
    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let job = adapter
        .poll_until_terminal(
            &service,
            job,
            RelaydanceVideoPollingConfig {
                max_attempts: 5,
                delay: Duration::from_millis(0),
            },
            |_| {},
            || 2,
        )
        .expect("poll");

    assert_eq!(job.status, MediaJobStatus::Succeeded);
    assert_eq!(job.artifact_ids.len(), 1);
    let artifact = service.load_artifact(&job.artifact_ids[0]).unwrap();
    assert!(artifact
        .path
        .starts_with(dir.path().join(".puffer/media/videos")));
}

#[test]
fn poll_url_preserves_submit_url_query() {
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations?token=x",
        "relaydance",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({})),
            polls: RefCell::new(vec![]),
            downloads: RefCell::new(Vec::new()),
        },
    );
    let mut job = MediaJob::new(
        "job-1".to_string(),
        MediaKind::Video,
        "relaydance",
        "m",
        "a cat",
        1,
        1,
    );
    job.provider_job_id = Some("vid-9".to_string());
    assert_eq!(
        adapter.poll_url(&job).expect("poll url"),
        "https://relaydance.com/v1/video/generations/vid-9?token=x"
    );
}

#[test]
fn submit_persists_selected_parameters() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({ "id": "vid-9", "status": "queued" })),
            polls: RefCell::new(vec![]),
            downloads: RefCell::new(Vec::new()),
        },
    );
    let request = RelaydanceVideoRequest {
        model: "m".into(),
        prompt: "a cat".into(),
        params: vec![],
        prompt_format: Default::default(),
    };
    let selected = BTreeMap::from([
        ("duration_seconds".to_string(), "5".to_string()),
        ("resolution".to_string(), "1080p".to_string()),
    ]);
    let job = adapter
        .submit(&service, request, selected.clone(), 1)
        .expect("submit");
    assert_eq!(job.parameters, selected);
}

#[test]
fn submit_then_poll_drives_new_api_lifecycle_to_success() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let transport = ScriptedTransport {
        submit: ScriptedJson::Ok(json!({ "data": { "id": "vid-9", "status": "NOT_START" } })),
        polls: RefCell::new(vec![
            json!({ "data": { "id": "vid-9", "status": "SUBMITTED" } }),
            json!({ "data": { "id": "vid-9", "status": "IN_PROGRESS" } }),
            json!({ "data": { "id": "vid-9", "status": "SUCCESS",
                "result_url": "https://cdn.example.com/v.mp4" } }),
        ]),
        downloads: RefCell::new(vec![ScriptedBytes::Ok(b"MP4BYTES".to_vec())]),
    };
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        transport,
    );
    let request = RelaydanceVideoRequest {
        model: "seedance-nsfw".into(),
        prompt: "a fleet battle".into(),
        params: vec![],
        prompt_format: Default::default(),
    };
    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    assert_eq!(job.status, MediaJobStatus::Queued);
    let job = adapter
        .poll_until_terminal(
            &service,
            job,
            RelaydanceVideoPollingConfig {
                max_attempts: 5,
                delay: Duration::from_millis(0),
            },
            |_| {},
            || 2,
        )
        .expect("poll");
    assert_eq!(job.status, MediaJobStatus::Succeeded);
    assert_eq!(job.artifact_ids.len(), 1);
}

#[test]
fn relaydance_poll_parser_failure_records_phase_context() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({ "id": "rd-task-1", "status": "queued" })),
            polls: RefCell::new(vec![json!({"status": "running"})]),
            downloads: RefCell::new(Vec::new()),
        },
    );
    let request = RelaydanceVideoRequest {
        model: "m".into(),
        prompt: "a cat".into(),
        params: vec![],
        prompt_format: Default::default(),
    };
    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let job = adapter
        .poll(&service, job, 2)
        .expect("poll returns Ok (transient)");
    let saved = service.load_job(&job.id).expect("saved job");
    let error = saved.error.as_deref().expect("poll error");

    assert!(!saved.status.is_terminal());
    assert!(error.contains("provider=relaydance"), "{error}");
    assert!(error.contains("adapter=relaydance_video"), "{error}");
    assert!(error.contains("phase=poll"), "{error}");
    assert!(error.contains("task=rd-task-1"), "{error}");
}

#[test]
fn relaydance_failed_poll_persists_remote_failure_diagnostics() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        tests_support::scripted(
            json!({"id": "rd-task-1", "status": "queued"}),
            vec![json!({
                "id": "rd-task-1",
                "status": "failed",
                "error": {"code": "blocked", "message": "content blocked upstream"}
            })],
        ),
    );
    let request = RelaydanceVideoRequest {
        model: "doubao-seedance-2-0-720p".into(),
        prompt: "a cat".into(),
        params: vec![],
        prompt_format: Default::default(),
    };

    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let job = adapter.poll(&service, job, 2).expect("poll");
    let persisted = service.load_job(&job.id).expect("persisted job");

    assert_eq!(persisted.status, MediaJobStatus::Failed);
    assert_eq!(persisted.provider_job_id.as_deref(), Some("rd-task-1"));
    assert_eq!(persisted.remote_status.as_deref(), Some("failed"));
    assert_eq!(persisted.error.as_deref(), Some("content blocked upstream"));
    assert_eq!(persisted, job);
}

#[test]
fn relaydance_download_failure_returns_media_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let service = MediaGenerationService::new(dir.path());
    let adapter = RelaydanceVideoAdapter::with_transport(
        "token",
        "https://relaydance.com/v1/video/generations",
        "relaydance",
        ScriptedTransport {
            submit: ScriptedJson::Ok(json!({"id": "rd-task-1", "status": "queued"})),
            polls: RefCell::new(vec![json!({
                "id": "rd-task-1",
                "status": "completed",
                "metadata": {"url": "https://media.example.com/out.mp4"}
            })]),
            downloads: RefCell::new(vec![ScriptedBytes::Err("cdn returned 503".to_string())]),
        },
    );
    let request = RelaydanceVideoRequest {
        model: "doubao-seedance-2-0-720p".into(),
        prompt: "a cat".into(),
        params: vec![],
        prompt_format: Default::default(),
    };

    let job = adapter
        .submit(&service, request, BTreeMap::new(), 1)
        .expect("submit");
    let error = adapter
        .poll_until_terminal(
            &service,
            job,
            RelaydanceVideoPollingConfig::default(),
            |_| {},
            || 2,
        )
        .expect_err("download should fail");
    let diagnostic = media_failure_diagnostic(&error).expect("diagnostic");

    assert_eq!(diagnostic.provider_id, "relaydance");
    assert_eq!(diagnostic.adapter.as_deref(), Some("relaydance_video"));
    assert_eq!(diagnostic.phase.as_deref(), Some("download"));
    assert_eq!(diagnostic.provider_job_id.as_deref(), Some("rd-task-1"));
}
