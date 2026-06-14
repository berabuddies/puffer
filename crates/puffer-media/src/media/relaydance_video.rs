use super::video_jobs::{
    complete_video_job, persist_failed_video_job, poll_video_until_terminal,
    video_job_failure_context, video_poll_url, video_request_failure_context,
    wrap_video_request_error, CompletedVideoTask, VideoPollingConfig,
};
use super::{MediaGenerationService, MediaJob, MediaJobStatus, MediaKind};
use crate::{media_failure_error, ProviderHttpError};
use anyhow::{anyhow, bail, Context, Result};
use puffer_provider_registry::{VideoPromptFormat, WireType};
use reqwest::blocking::Client;
use serde_json::{json, Map, Number, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

/// One Relaydance video generation request (`POST /v1/video/generations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct RelaydanceVideoRequest {
    pub(crate) model: String,
    pub(crate) prompt: String,
    /// Ordered (request_field, value) pairs. A `request_field` of `metadata.<k>`
    /// is nested under the body's `metadata` object; otherwise it is top-level.
    pub(crate) params: Vec<(String, Value)>,
    pub(crate) prompt_format: VideoPromptFormat,
}

const METADATA_PREFIX: &str = "metadata.";

impl RelaydanceVideoRequest {
    fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            bail!("video model is required");
        }
        if self.prompt.trim().is_empty() {
            bail!("video prompt is required");
        }
        Ok(())
    }

    /// Builds the video generation request body.
    pub(crate) fn request_body(&self) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model.trim()));
        match self.prompt_format {
            VideoPromptFormat::ContentArray => {
                body.insert(
                    "content".to_string(),
                    json!([{"type": "text", "text": self.prompt.trim()}]),
                );
            }
            VideoPromptFormat::Prompt => {
                body.insert("prompt".to_string(), json!(self.prompt.trim()));
                body.insert("n".to_string(), json!(1));
            }
        }

        let mut metadata = Map::new();
        for (field, value) in &self.params {
            let field = field.trim();
            if let Some(key) = field.strip_prefix(METADATA_PREFIX) {
                metadata.insert(key.to_string(), value.clone());
            } else {
                body.insert(field.to_string(), value.clone());
            }
        }
        if !metadata.is_empty() {
            body.insert("metadata".to_string(), Value::Object(metadata));
        }
        Value::Object(body)
    }
}

/// Maps resolved request parameters (keyed by upstream request field) into a
/// Relaydance video request. A `metadata.<k>` field is nested under the body's
/// `metadata` object; every other field is top-level.
pub(crate) fn relaydance_video_request_from_parameters(
    model_id: String,
    prompt: String,
    parameters: &BTreeMap<String, String>,
    parameter_wire_types: &BTreeMap<String, WireType>,
    prompt_format: VideoPromptFormat,
) -> Result<RelaydanceVideoRequest> {
    let params = parameters
        .iter()
        .map(|(field, value)| {
            let wire_type = parameter_wire_types.get(field).copied().unwrap_or_default();
            Ok((
                field.clone(),
                relaydance_request_value(field, value, wire_type)?,
            ))
        })
        .collect::<Result<Vec<_>>>()?;
    let request = RelaydanceVideoRequest {
        model: model_id,
        prompt,
        params,
        prompt_format,
    };
    request.validate()?;
    Ok(request)
}

fn relaydance_request_value(field: &str, value: &str, wire_type: WireType) -> Result<Value> {
    let value = value.trim();
    match wire_type {
        WireType::String => Ok(json!(value)),
        WireType::Number => relaydance_number_value(field, value),
    }
}

fn relaydance_number_value(field: &str, value: &str) -> Result<Value> {
    if let Ok(number) = value.parse::<i64>() {
        return Ok(Value::Number(Number::from(number)));
    }
    let number = value
        .parse::<f64>()
        .with_context(|| format!("video generation parameter {field} must be numeric"))?;
    Number::from_f64(number)
        .map(Value::Number)
        .with_context(|| format!("video generation parameter {field} must be finite"))
}

/// Abstracts Relaydance video HTTP operations for production and tests.
pub(crate) trait RelaydanceVideoTransport {
    /// Submits a video task and returns its JSON response.
    fn submit_task(&self, url: &str, api_token: &str, body: &Value) -> Result<Value>;

    /// Polls a video task URL and returns its JSON response.
    fn poll_task(&self, url: &str, api_token: &str) -> Result<Value>;

    /// Downloads the rendered video bytes from a validated remote URL.
    fn download_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

/// Reqwest-backed transport used by the runtime adapter.
#[derive(Debug, Clone, Default)]
pub(crate) struct ReqwestRelaydanceVideoTransport {
    client: Client,
}

impl RelaydanceVideoTransport for ReqwestRelaydanceVideoTransport {
    fn submit_task(&self, url: &str, api_token: &str, body: &Value) -> Result<Value> {
        let response = self
            .client
            .post(url)
            .bearer_auth(api_token)
            .json(body)
            .send()
            .with_context(|| format!("submit video task {url}"))?;
        relaydance_video_json_response(response, "submit video task")
    }

    fn poll_task(&self, url: &str, api_token: &str) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .bearer_auth(api_token)
            .send()
            .with_context(|| format!("poll video task {url}"))?;
        relaydance_video_json_response(response, "poll video task")
    }

    fn download_bytes(&self, url: &str) -> Result<Vec<u8>> {
        // Reuse the shared https/loopback-enforcing downloader (image-path parity).
        super::http_support::download_image_url(&self.client, url, "video output")
    }
}

fn relaydance_video_json_response(
    response: reqwest::blocking::Response,
    label: &str,
) -> Result<Value> {
    let status = response.status();
    let text = response
        .text()
        .with_context(|| format!("read {label} response body"))?;
    if !status.is_success() {
        return Err(anyhow::Error::new(ProviderHttpError::new(
            label,
            status.as_u16(),
            text,
        )));
    }
    serde_json::from_str(&text).with_context(|| format!("parse {label} response JSON"))
}

/// Normalized view of a Relaydance video task response.
///
/// Envelope confirmed from New API `dto/relaydance_video.go`: `id`, `status`
/// (`queued|in_progress|completed|failed`), the video URL at `metadata.url`,
/// and `error.{message,code}`.
#[derive(Debug, Clone)]
pub(crate) struct RelaydanceVideoTask {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) video_url: Option<String>,
    pub(crate) error: Option<String>,
}

impl RelaydanceVideoTask {
    pub(crate) fn from_value(value: Value, phase: &str) -> Result<Self> {
        let body = value.get("data").unwrap_or(&value);
        let id = string_field(body, &["id", "task_id"])
            .or_else(|| string_field(&value, &["id", "task_id"]))
            .ok_or_else(|| {
                anyhow!(
                    "{phase} response missing task id: {}",
                    relaydance_response_shape_summary(&value)
                )
            })?;
        let status = string_field(body, &["status"])
            .or_else(|| string_field(&value, &["status"]))
            .ok_or_else(|| {
                anyhow!(
                    "{phase} response missing status: {}",
                    relaydance_response_shape_summary(&value)
                )
            })?;
        let video_url = relaydance_video_url(body).or_else(|| relaydance_video_url(&value));
        let error = relaydance_error_message(body).or_else(|| relaydance_error_message(&value));
        Ok(Self {
            id,
            status,
            video_url,
            error,
        })
    }

    /// Maps the Relaydance status string onto a media job status (infallible;
    /// unknown statuses stay non-terminal — see `map_video_task_status`).
    pub(crate) fn media_status(&self) -> MediaJobStatus {
        super::video_jobs::map_video_task_status(&self.status)
    }
}

fn relaydance_response_shape_summary(value: &serde_json::Value) -> String {
    let keys = value
        .as_object()
        .map(|object| {
            let mut keys = object.keys().cloned().collect::<Vec<_>>();
            keys.sort();
            keys.join(",")
        })
        .unwrap_or_else(|| value_type_name(value).to_string());
    format!("keys=[{keys}]")
}

fn value_type_name(value: &serde_json::Value) -> &'static str {
    match value {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "number",
        serde_json::Value::String(_) => "string",
        serde_json::Value::Array(_) => "array",
        serde_json::Value::Object(_) => "object",
    }
}

fn relaydance_error_message(value: &serde_json::Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .or_else(|| value.get("fail_reason"))
        .or_else(|| value.get("message"))
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|message| !message.is_empty())
        .map(str::to_string)
}

fn string_field(value: &Value, names: &[&str]) -> Option<String> {
    names.iter().find_map(|name| {
        value
            .get(*name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_string)
    })
}

fn relaydance_video_url(value: &Value) -> Option<String> {
    value
        .get("metadata")
        .and_then(|metadata| metadata.get("url"))
        .or_else(|| value.get("url"))
        .or_else(|| value.get("video_url"))
        .or_else(|| value.get("result_url"))
        .or_else(|| {
            value
                .get("content")
                .and_then(|content| content.get("video_url"))
        })
        .or_else(|| {
            value
                .get("data")
                .and_then(|data| data.get("content"))
                .and_then(|content| content.get("video_url"))
        })
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) const RELAYDANCE_VIDEO_ADAPTER: &str = "relaydance_video";

pub(crate) type RelaydanceVideoPollingConfig = VideoPollingConfig;

/// Submits and polls Relaydance video tasks into media jobs.
pub(crate) struct RelaydanceVideoAdapter<T = ReqwestRelaydanceVideoTransport> {
    api_token: String,
    submit_url: String,
    provider_id: String,
    transport: T,
}

impl RelaydanceVideoAdapter<ReqwestRelaydanceVideoTransport> {
    /// Creates a production adapter. `submit_url` is the absolute task-creation
    /// URL built by the caller via `provider_execution_url`.
    pub(crate) fn new(
        api_token: impl Into<String>,
        submit_url: impl Into<String>,
        provider_id: impl Into<String>,
    ) -> Result<Self> {
        let api_token = api_token.into().trim().to_string();
        if api_token.is_empty() {
            bail!("video API token is required");
        }
        Ok(Self {
            api_token,
            submit_url: submit_url.into().trim_end_matches('/').to_string(),
            provider_id: provider_id.into(),
            transport: ReqwestRelaydanceVideoTransport::default(),
        })
    }
}

impl<T> RelaydanceVideoAdapter<T>
where
    T: RelaydanceVideoTransport,
{
    #[cfg(test)]
    pub(crate) fn with_transport(
        api_token: impl Into<String>,
        submit_url: impl Into<String>,
        provider_id: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            api_token: api_token.into().trim().to_string(),
            submit_url: submit_url.into().trim_end_matches('/').to_string(),
            provider_id: provider_id.into(),
            transport,
        }
    }

    /// Submits a task and persists the queued job (task id in `provider_job_id`).
    ///
    /// `selected_parameters` are canonical user-facing name/value pairs such
    /// as `duration_seconds`, `resolution`, and `aspect_ratio`.
    pub(crate) fn submit(
        &self,
        service: &MediaGenerationService,
        request: RelaydanceVideoRequest,
        selected_parameters: BTreeMap<String, String>,
        now_ms: u64,
    ) -> Result<MediaJob> {
        let response = self
            .transport
            .submit_task(&self.submit_url, &self.api_token, &request.request_body())
            .map_err(|error| {
                wrap_video_request_error(
                    &self.provider_id,
                    RELAYDANCE_VIDEO_ADAPTER,
                    &request.model,
                    "submit",
                    error,
                )
            })?;
        let task =
            RelaydanceVideoTask::from_value(response, "submit video task").map_err(|error| {
                wrap_video_request_error(
                    &self.provider_id,
                    RELAYDANCE_VIDEO_ADAPTER,
                    &request.model,
                    "submit",
                    error,
                )
            })?;
        let mut job = MediaJob::new(
            Uuid::new_v4().to_string(),
            MediaKind::Video,
            &self.provider_id,
            request.model.trim(),
            request.prompt.trim(),
            1,
            now_ms,
        );
        job.adapter = Some(RELAYDANCE_VIDEO_ADAPTER.to_string());
        job.parameters = selected_parameters;
        job.provider_job_id = Some(task.id.clone());
        self.apply_task(service, job, task, now_ms)
    }

    fn poll_url(&self, job: &MediaJob) -> Result<String> {
        video_poll_url(&self.submit_url, job)
    }

    /// Fetches and parses the current remote task state for a job. Any failure
    /// (URL build, transport, or parse) surfaces as an `Err` for the caller to
    /// treat as transient.
    fn fetch_task(&self, job: &MediaJob) -> Result<RelaydanceVideoTask> {
        let url = self.poll_url(job)?;
        let response = self.transport.poll_task(&url, &self.api_token)?;
        RelaydanceVideoTask::from_value(response, "poll video task")
    }

    /// Polls a non-terminal job once and persists the resulting state.
    ///
    /// URL, transport, and parse failures are treated as transient: the error
    /// is recorded on the job but its status stays non-terminal, so the polling
    /// loop retries within its attempt budget instead of aborting the job.
    pub(crate) fn poll(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        now_ms: u64,
    ) -> Result<MediaJob> {
        if job.status.is_terminal() {
            return Ok(job);
        }
        match self.fetch_task(&job) {
            Ok(task) => self.apply_task(service, job, task, now_ms),
            Err(error) => {
                let label = format!(
                    "provider={} adapter={RELAYDANCE_VIDEO_ADAPTER} phase=poll task={}",
                    self.provider_id,
                    job.provider_job_id.as_deref().unwrap_or("unknown")
                );
                let diagnostic = media_failure_error(
                    video_job_failure_context(
                        &self.provider_id,
                        RELAYDANCE_VIDEO_ADAPTER,
                        &job,
                        "poll",
                    ),
                    error.context(label),
                );
                super::video_jobs::record_transient_poll_error(service, job, diagnostic, now_ms)
            }
        }
    }

    /// Polls until the job reaches a terminal status.
    pub(crate) fn poll_until_terminal(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        config: RelaydanceVideoPollingConfig,
        sleep: impl FnMut(Duration),
        now_ms: impl FnMut() -> u64,
    ) -> Result<MediaJob> {
        poll_video_until_terminal(job, config, sleep, now_ms, |job, now_ms| {
            self.poll(service, job, now_ms)
        })
    }

    fn apply_task(
        &self,
        service: &MediaGenerationService,
        mut job: MediaJob,
        task: RelaydanceVideoTask,
        now_ms: u64,
    ) -> Result<MediaJob> {
        let status = task.media_status();
        job.remote_status = Some(task.status.clone());
        match status {
            MediaJobStatus::Queued | MediaJobStatus::Running => {
                job.transition(status, now_ms)?;
                service.save_job(&job)?;
                Ok(job)
            }
            MediaJobStatus::Succeeded => self.complete_succeeded(service, job, &task, now_ms),
            MediaJobStatus::Failed => persist_failed_video_job(
                service,
                job,
                task.error
                    .clone()
                    .unwrap_or_else(|| "video task failed".to_string()),
                now_ms,
            ),
            MediaJobStatus::Canceled => {
                job.transition(MediaJobStatus::Canceled, now_ms)?;
                service.save_job(&job)?;
                Ok(job)
            }
        }
    }

    fn complete_succeeded(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        task: &RelaydanceVideoTask,
        now_ms: u64,
    ) -> Result<MediaJob> {
        let model_id = job.model_id.clone();
        let provider_job_id = job.provider_job_id.clone();
        let remote_status = task.status.clone();
        let task_id = task.id.clone();
        complete_video_job(
            service,
            job,
            CompletedVideoTask {
                provider_id: &self.provider_id,
                task_id: &task.id,
                remote_status: &task.status,
                video_url: task.video_url.as_deref(),
                filename_prefix: "relaydance-video",
                missing_url_message: "completed video task is missing `metadata.url`",
            },
            now_ms,
            |url| {
                let mut context = video_request_failure_context(
                    &self.provider_id,
                    RELAYDANCE_VIDEO_ADAPTER,
                    &model_id,
                    "download",
                )
                .remote_status(remote_status);
                if let Some(provider_job_id) = &provider_job_id {
                    context = context.provider_job_id(provider_job_id.clone());
                }
                let label = format!(
                    "provider={} adapter={RELAYDANCE_VIDEO_ADAPTER} phase=download task={task_id}",
                    self.provider_id
                );
                self.transport
                    .download_bytes(url)
                    .map_err(|error| media_failure_error(context, error.context(label)))
            },
        )
    }
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::RelaydanceVideoTransport;
    use crate::ProviderHttpError;
    use anyhow::{anyhow, Result};
    use serde_json::Value;
    use std::cell::RefCell;

    #[derive(Clone)]
    pub(crate) enum ScriptedJson {
        Ok(Value),
        Err(ProviderHttpError),
    }

    impl ScriptedJson {
        fn result(&self) -> Result<Value> {
            match self {
                Self::Ok(value) => Ok(value.clone()),
                Self::Err(error) => Err(anyhow::Error::new(error.clone())),
            }
        }
    }

    #[derive(Clone)]
    pub(crate) enum ScriptedBytes {
        Ok(Vec<u8>),
        Err(String),
    }

    impl ScriptedBytes {
        fn result(&self) -> Result<Vec<u8>> {
            match self {
                Self::Ok(bytes) => Ok(bytes.clone()),
                Self::Err(error) => Err(anyhow!(error.clone())),
            }
        }
    }

    /// Scripted transport returning canned submit/poll responses in tests.
    pub(crate) struct ScriptedTransport {
        pub(crate) submit: ScriptedJson,
        pub(crate) polls: RefCell<Vec<Value>>,
        pub(crate) downloads: RefCell<Vec<ScriptedBytes>>,
    }

    impl RelaydanceVideoTransport for ScriptedTransport {
        fn submit_task(&self, _url: &str, _token: &str, _body: &Value) -> Result<Value> {
            self.submit.result()
        }
        fn poll_task(&self, _url: &str, _token: &str) -> Result<Value> {
            Ok(self.polls.borrow_mut().remove(0))
        }
        fn download_bytes(&self, _url: &str) -> Result<Vec<u8>> {
            let mut downloads = self.downloads.borrow_mut();
            if downloads.is_empty() {
                return Ok(b"MP4BYTES".to_vec());
            }
            downloads.remove(0).result()
        }
    }

    /// Builds a scripted transport from a submit response and ordered polls.
    pub(crate) fn scripted(submit: Value, polls: Vec<Value>) -> ScriptedTransport {
        ScriptedTransport {
            submit: ScriptedJson::Ok(submit),
            polls: RefCell::new(polls),
            downloads: RefCell::new(vec![ScriptedBytes::Ok(b"MP4BYTES".to_vec())]),
        }
    }

    pub(crate) fn scripted_submit_error(error: ProviderHttpError) -> ScriptedTransport {
        ScriptedTransport {
            submit: ScriptedJson::Err(error),
            polls: RefCell::new(Vec::new()),
            downloads: RefCell::new(Vec::new()),
        }
    }
}

#[cfg(test)]
#[path = "relaydance_video_tests.rs"]
mod tests;
