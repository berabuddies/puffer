use super::video_jobs::{
    complete_video_job, persist_failed_video_job, poll_video_until_terminal,
    video_job_failure_context, video_poll_url, video_request_failure_context,
    wrap_video_request_error, CompletedVideoTask, VideoPollingConfig,
};
use super::{MediaGenerationService, MediaJob, MediaJobStatus, MediaKind};
use crate::{media_failure_error, ProviderHttpError};
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::time::Duration;
use uuid::Uuid;

pub(crate) const BYTEPLUS_VIDEO_ADAPTER: &str = "byteplus_video";

#[derive(Debug)]
pub(crate) struct BytePlusVideoRequest {
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) image_references: Vec<String>,
    pub(crate) params: Vec<(String, Value)>,
}

impl BytePlusVideoRequest {
    /// Builds a BytePlus ModelArk text-to-video task request.
    pub(crate) fn request_body(&self) -> Value {
        let mut body = Map::new();
        body.insert("model".to_string(), json!(self.model.trim()));
        let mut content = vec![json!({
            "type": "text",
            "text": self.prompt.trim()
        })];
        for reference in &self.image_references {
            content.push(json!({
                "type": "image_url",
                "image_url": {
                    "url": reference.trim()
                }
            }));
        }
        body.insert("content".to_string(), Value::Array(content));
        // Default to silent video. BytePlus Seedance models default
        // `generate_audio` to true, and their auto-generated audio reliably
        // trips content moderation ("output audio may contain sensitive
        // information"), failing every job — even benign prompts. Inserted
        // before params so a future `generate_audio` parameter can override.
        body.insert("generate_audio".to_string(), Value::Bool(false));
        for (field, value) in &self.params {
            body.insert(field.trim().to_string(), value.clone());
        }
        Value::Object(body)
    }

    fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            bail!("video model is required");
        }
        if self.prompt.trim().is_empty() {
            bail!("video prompt is required");
        }
        for (index, reference) in self.image_references.iter().enumerate() {
            if reference.trim().is_empty() {
                bail!("video image reference {index} is empty");
            }
        }
        Ok(())
    }
}

/// BytePlus request fields that must be encoded as JSON numbers. BytePlus
/// requires `duration` to be numeric (the fast model rejects a string
/// duration); every other field (`ratio`, `resolution`) is a non-numeric
/// string. Encoding by an explicit allowlist — rather than parsing every value
/// — avoids mis-encoding a future numeric-looking string field.
const BYTEPLUS_NUMERIC_FIELDS: &[&str] = &["duration"];

/// Encodes one resolved BytePlus parameter value as JSON, as a number only for
/// fields in [`BYTEPLUS_NUMERIC_FIELDS`].
fn byteplus_request_value(field: &str, value: &str) -> Value {
    let value = value.trim();
    if BYTEPLUS_NUMERIC_FIELDS.contains(&field) {
        if let Ok(number) = value.parse::<u64>() {
            return Value::from(number);
        }
    }
    json!(value)
}

/// Maps resolved request parameters (keyed by upstream request field) into a
/// BytePlus text-to-video request.
pub(crate) fn byteplus_video_request_from_parameters(
    model_id: String,
    prompt: String,
    image_references: Vec<String>,
    parameters: &BTreeMap<String, String>,
) -> Result<BytePlusVideoRequest> {
    let params = parameters
        .iter()
        .map(|(field, value)| (field.clone(), byteplus_request_value(field, value)))
        .collect();
    let request = BytePlusVideoRequest {
        model: model_id,
        prompt,
        image_references,
        params,
    };
    request.validate()?;
    Ok(request)
}

pub(crate) struct BytePlusVideoTask {
    pub(crate) id: String,
    pub(crate) status: String,
    pub(crate) video_url: Option<String>,
    pub(crate) error: Option<String>,
}

impl BytePlusVideoTask {
    /// Parses the BytePlus task creation response.
    pub(crate) fn from_submit_value(value: Value) -> Result<Self> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("submit video task response missing task id")?
            .to_string();
        Ok(Self {
            id,
            status: "queued".to_string(),
            video_url: None,
            error: byteplus_error_message(&value),
        })
    }

    /// Parses the BytePlus task retrieval response.
    pub(crate) fn from_poll_value(value: Value) -> Result<Self> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("poll video task response missing task id")?
            .to_string();
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("poll video task response missing status")?
            .to_string();
        Ok(Self {
            id,
            status,
            video_url: byteplus_video_url(&value),
            error: byteplus_error_message(&value),
        })
    }

    fn media_status(&self) -> MediaJobStatus {
        super::video_jobs::map_video_task_status(&self.status)
    }
}

fn byteplus_video_url(value: &Value) -> Option<String> {
    value
        .get("content")
        .and_then(|content| content.get("video_url"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn byteplus_error_message(value: &Value) -> Option<String> {
    value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(Value::as_str)
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

pub(crate) trait BytePlusVideoTransport {
    /// Submits a BytePlus video task and returns its JSON response.
    fn submit_task(&self, url: &str, api_token: &str, body: &Value) -> Result<Value>;

    /// Polls a BytePlus video task URL and returns its JSON response.
    fn poll_task(&self, url: &str, api_token: &str) -> Result<Value>;

    /// Downloads rendered video bytes from a validated remote URL.
    fn download_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

#[derive(Debug, Clone, Default)]
pub(crate) struct ReqwestBytePlusVideoTransport {
    client: Client,
}

impl BytePlusVideoTransport for ReqwestBytePlusVideoTransport {
    fn submit_task(&self, url: &str, api_token: &str, body: &Value) -> Result<Value> {
        let response = self
            .client
            .post(url)
            .bearer_auth(api_token)
            .json(body)
            .send()
            .with_context(|| format!("submit video task {url}"))?;
        byteplus_video_json_response(response, "submit video task")
    }

    fn poll_task(&self, url: &str, api_token: &str) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .bearer_auth(api_token)
            .send()
            .with_context(|| format!("poll video task {url}"))?;
        byteplus_video_json_response(response, "poll video task")
    }

    fn download_bytes(&self, url: &str) -> Result<Vec<u8>> {
        super::http_support::download_image_url(&self.client, url, "video output")
    }
}

fn byteplus_video_json_response(
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

pub(crate) type BytePlusVideoPollingConfig = VideoPollingConfig;

pub(crate) struct BytePlusVideoAdapter<T = ReqwestBytePlusVideoTransport> {
    api_token: String,
    submit_url: String,
    provider_id: String,
    transport: T,
}

impl BytePlusVideoAdapter<ReqwestBytePlusVideoTransport> {
    /// Creates a production BytePlus video adapter.
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
            transport: ReqwestBytePlusVideoTransport::default(),
        })
    }
}

impl<T> BytePlusVideoAdapter<T>
where
    T: BytePlusVideoTransport,
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

    /// Submits a BytePlus task and persists the queued job.
    pub(crate) fn submit(
        &self,
        service: &MediaGenerationService,
        request: BytePlusVideoRequest,
        selected_parameters: BTreeMap<String, String>,
        now_ms: u64,
    ) -> Result<MediaJob> {
        let response = self
            .transport
            .submit_task(&self.submit_url, &self.api_token, &request.request_body())
            .map_err(|error| {
                wrap_video_request_error(
                    &self.provider_id,
                    BYTEPLUS_VIDEO_ADAPTER,
                    &request.model,
                    "submit",
                    error,
                )
            })?;
        let task = BytePlusVideoTask::from_submit_value(response).map_err(|error| {
            wrap_video_request_error(
                &self.provider_id,
                BYTEPLUS_VIDEO_ADAPTER,
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
        job.adapter = Some(BYTEPLUS_VIDEO_ADAPTER.to_string());
        job.parameters = selected_parameters;
        job.provider_job_id = Some(task.id.clone());
        self.apply_task(service, job, task, now_ms)
    }

    /// Fetches and parses the current remote task state for a job. Any failure
    /// (transport or parse) surfaces as an `Err` for the caller to treat as
    /// transient.
    fn fetch_task(&self, job: &MediaJob) -> Result<BytePlusVideoTask> {
        let url = video_poll_url(&self.submit_url, job)?;
        let response = self.transport.poll_task(&url, &self.api_token)?;
        BytePlusVideoTask::from_poll_value(response)
    }

    /// Polls a non-terminal BytePlus job once and persists the resulting state.
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
                    "provider={} adapter={BYTEPLUS_VIDEO_ADAPTER} phase=poll task={}",
                    self.provider_id,
                    job.provider_job_id.as_deref().unwrap_or("unknown")
                );
                let diagnostic = media_failure_error(
                    video_job_failure_context(
                        &self.provider_id,
                        BYTEPLUS_VIDEO_ADAPTER,
                        &job,
                        "poll",
                    ),
                    error.context(label),
                );
                super::video_jobs::record_transient_poll_error(service, job, diagnostic, now_ms)
            }
        }
    }

    /// Polls until a BytePlus job reaches a terminal status.
    pub(crate) fn poll_until_terminal(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        config: BytePlusVideoPollingConfig,
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
        task: BytePlusVideoTask,
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
        task: &BytePlusVideoTask,
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
                filename_prefix: "byteplus-video",
                missing_url_message: "completed video task is missing content.video_url",
            },
            now_ms,
            |url| {
                let mut context = video_request_failure_context(
                    &self.provider_id,
                    BYTEPLUS_VIDEO_ADAPTER,
                    &model_id,
                    "download",
                )
                .remote_status(remote_status);
                if let Some(provider_job_id) = &provider_job_id {
                    context = context.provider_job_id(provider_job_id.clone());
                }
                let label = format!(
                    "provider={} adapter={BYTEPLUS_VIDEO_ADAPTER} phase=download task={task_id}",
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
    use super::BytePlusVideoTransport;
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

    impl BytePlusVideoTransport for ScriptedTransport {
        fn submit_task(&self, _url: &str, _api_token: &str, _body: &Value) -> Result<Value> {
            self.submit.result()
        }

        fn poll_task(&self, _url: &str, _api_token: &str) -> Result<Value> {
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
#[path = "byteplus_video_tests.rs"]
mod tests;
