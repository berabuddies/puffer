use super::video_jobs::{complete_video_job, CompletedVideoTask};
use super::{MediaGenerationService, MediaJob, MediaJobStatus, MediaKind};
use anyhow::{bail, Context, Result};
use reqwest::blocking::Client;
use serde_json::{json, Value};
use std::collections::BTreeMap;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const REPLICATE_PROVIDER_ID: &str = "replicate";
const DEFAULT_REPLICATE_BASE_URL: &str = "https://api.replicate.com";

/// Describes a Replicate prediction request for video generation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ReplicateVideoRequest {
    pub(crate) model: String,
    pub(crate) prompt: String,
    pub(crate) aspect_ratio: String,
    pub(crate) duration_seconds: u32,
}

impl ReplicateVideoRequest {
    fn validate(&self) -> Result<()> {
        if self.model.trim().is_empty() {
            bail!("Replicate video model is required");
        }
        if self.prompt.trim().is_empty() {
            bail!("Replicate video prompt is required");
        }
        if self.aspect_ratio.trim().is_empty() {
            bail!("Replicate video aspect ratio is required");
        }
        if self.duration_seconds == 0 {
            bail!("Replicate video duration must be greater than zero");
        }
        Ok(())
    }

    fn request_body(&self) -> Value {
        json!({
            "version": self.model.trim(),
            "input": {
                "prompt": self.prompt.trim(),
                "aspect_ratio": self.aspect_ratio.trim(),
                "duration": self.duration_seconds
            }
        })
    }
}

/// Controls bounded backoff while polling Replicate predictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReplicatePollingConfig {
    max_attempts: usize,
    initial_delay_ms: u64,
    max_delay_ms: u64,
}

impl ReplicatePollingConfig {
    /// Creates a polling configuration with at least one attempt.
    pub(crate) fn new(max_attempts: usize, initial_delay_ms: u64, max_delay_ms: u64) -> Self {
        Self {
            max_attempts: max_attempts.max(1),
            initial_delay_ms,
            max_delay_ms: max_delay_ms.max(initial_delay_ms),
        }
    }

    fn delay_for_attempt(self, attempt: usize) -> Duration {
        let multiplier = 1_u64.checked_shl(attempt as u32).unwrap_or(u64::MAX);
        let delay = self
            .initial_delay_ms
            .saturating_mul(multiplier)
            .min(self.max_delay_ms);
        Duration::from_millis(delay)
    }
}

impl Default for ReplicatePollingConfig {
    fn default() -> Self {
        Self::new(120, 1_000, 30_000)
    }
}

/// Abstracts Replicate HTTP operations for production and tests.
pub(crate) trait ReplicateVideoTransport {
    /// Submits a new Replicate prediction and returns its JSON response.
    fn submit_prediction(&self, base_url: &str, api_token: &str, body: &Value) -> Result<Value>;

    /// Polls a Replicate prediction URL and returns its JSON response.
    fn poll_prediction(&self, url: &str, api_token: &str) -> Result<Value>;

    /// Downloads provider output bytes from a validated remote URL.
    fn download_bytes(&self, url: &str) -> Result<Vec<u8>>;
}

/// Reqwest-backed Replicate transport used by the runtime adapter.
#[derive(Debug, Clone)]
pub(crate) struct ReqwestReplicateVideoTransport {
    client: Client,
}

impl Default for ReqwestReplicateVideoTransport {
    fn default() -> Self {
        Self {
            client: Client::new(),
        }
    }
}

impl ReplicateVideoTransport for ReqwestReplicateVideoTransport {
    fn submit_prediction(&self, base_url: &str, api_token: &str, body: &Value) -> Result<Value> {
        let url = format!("{}/v1/predictions", base_url.trim_end_matches('/'));
        let response = self
            .client
            .post(&url)
            .bearer_auth(api_token)
            .json(body)
            .send()
            .with_context(|| format!("submit Replicate video prediction {url}"))?;
        json_response(response, "submit Replicate video prediction")
    }

    fn poll_prediction(&self, url: &str, api_token: &str) -> Result<Value> {
        let response = self
            .client
            .get(url)
            .bearer_auth(api_token)
            .send()
            .with_context(|| format!("poll Replicate video prediction {url}"))?;
        json_response(response, "poll Replicate video prediction")
    }

    fn download_bytes(&self, url: &str) -> Result<Vec<u8>> {
        let parsed =
            reqwest::Url::parse(url).context("Replicate video output URL must be absolute")?;
        if parsed.scheme() != "https" {
            bail!(
                "unsupported Replicate video output URL scheme `{}`",
                parsed.scheme()
            );
        }
        let response = self
            .client
            .get(parsed)
            .send()
            .context("download Replicate video output")?;
        let status = response.status();
        if !status.is_success() {
            bail!(
                "download Replicate video output failed with status {}",
                status.as_u16()
            );
        }
        Ok(response
            .bytes()
            .context("read Replicate video output bytes")?
            .to_vec())
    }
}

/// Normalizes Replicate prediction lifecycle events into media jobs.
pub(crate) struct ReplicateVideoAdapter<T = ReqwestReplicateVideoTransport> {
    api_token: String,
    base_url: String,
    transport: T,
}

impl ReplicateVideoAdapter<ReqwestReplicateVideoTransport> {
    /// Creates a production Replicate video adapter using the default API base URL.
    pub(crate) fn new(api_token: impl Into<String>) -> Result<Self> {
        let api_token = api_token.into();
        if api_token.trim().is_empty() {
            bail!("Replicate API token is required");
        }
        Ok(Self {
            api_token: api_token.trim().to_string(),
            base_url: DEFAULT_REPLICATE_BASE_URL.to_string(),
            transport: ReqwestReplicateVideoTransport::default(),
        })
    }
}

impl<T> ReplicateVideoAdapter<T>
where
    T: ReplicateVideoTransport,
{
    /// Creates an adapter with an explicit transport and base URL.
    #[cfg(test)]
    pub(crate) fn with_transport(
        api_token: impl Into<String>,
        base_url: impl Into<String>,
        transport: T,
    ) -> Self {
        Self {
            api_token: api_token.into().trim().to_string(),
            base_url: base_url.into().trim_end_matches('/').to_string(),
            transport,
        }
    }

    /// Submits a video prediction and persists the initial job state.
    pub(crate) fn submit(
        &self,
        service: &MediaGenerationService,
        request: ReplicateVideoRequest,
        now_ms: u64,
    ) -> Result<MediaJob> {
        request.validate()?;
        let body = request.request_body();
        let response = self
            .transport
            .submit_prediction(&self.base_url, &self.api_token, &body)?;
        let prediction = ReplicatePrediction::from_value(response)?;
        let mut job = MediaJob::new(
            Uuid::new_v4().to_string(),
            MediaKind::Video,
            REPLICATE_PROVIDER_ID,
            request.model.trim(),
            request.prompt.trim(),
            1,
            now_ms,
        );
        job.adapter = Some("replicate_video".to_string());
        job.parameters = BTreeMap::from([
            ("aspect_ratio".to_string(), request.aspect_ratio.clone()),
            ("duration".to_string(), request.duration_seconds.to_string()),
        ]);
        apply_remote_prediction_metadata(&mut job, &prediction);
        self.apply_prediction(service, job, prediction, now_ms)
    }

    /// Polls a non-terminal job once and persists the resulting state.
    pub(crate) fn poll(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        now_ms: u64,
    ) -> Result<MediaJob> {
        if job.status.is_terminal() {
            return Ok(job);
        }
        let poll_url = job
            .remote_get_url
            .clone()
            .or_else(|| {
                job.provider_job_id
                    .as_ref()
                    .map(|id| format!("{}/v1/predictions/{id}", self.base_url))
            })
            .context("Replicate video job is missing a poll URL")?;
        let response = self.transport.poll_prediction(&poll_url, &self.api_token)?;
        let prediction = ReplicatePrediction::from_value(response)?;
        self.apply_prediction(service, job, prediction, now_ms)
    }

    /// Polls a job with default sleeping until it reaches a terminal state.
    pub(crate) fn poll_until_terminal(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        config: ReplicatePollingConfig,
    ) -> Result<MediaJob> {
        self.poll_until_terminal_with_sleep(service, job, config, now_ms, thread::sleep)
    }

    /// Polls a job until terminal using injected time and sleep functions.
    pub(crate) fn poll_until_terminal_with_sleep<N, S>(
        &self,
        service: &MediaGenerationService,
        mut job: MediaJob,
        config: ReplicatePollingConfig,
        mut now_ms: N,
        mut sleep: S,
    ) -> Result<MediaJob>
    where
        N: FnMut() -> u64,
        S: FnMut(Duration),
    {
        for attempt in 0..config.max_attempts {
            job = self.poll(service, job, now_ms())?;
            if job.status.is_terminal() {
                return Ok(job);
            }
            if attempt + 1 < config.max_attempts {
                sleep(config.delay_for_attempt(attempt));
            }
        }
        bail!(
            "Replicate video job `{}` did not reach a terminal status after {} polls",
            job.id,
            config.max_attempts
        )
    }

    fn apply_prediction(
        &self,
        service: &MediaGenerationService,
        mut job: MediaJob,
        prediction: ReplicatePrediction,
        now_ms: u64,
    ) -> Result<MediaJob> {
        apply_remote_prediction_metadata(&mut job, &prediction);
        let status = prediction.media_status()?;
        match status {
            MediaJobStatus::Queued | MediaJobStatus::Running => {
                job.transition(status, now_ms)?;
                service.save_job(&job)?;
                Ok(job)
            }
            MediaJobStatus::Succeeded => {
                self.complete_succeeded_prediction(service, job, &prediction, now_ms)
            }
            MediaJobStatus::Failed => {
                job.error = prediction.error.clone();
                job.transition(MediaJobStatus::Failed, now_ms)?;
                service.save_job(&job)?;
                Ok(job)
            }
            MediaJobStatus::Canceled => {
                job.transition(MediaJobStatus::Canceled, now_ms)?;
                service.save_job(&job)?;
                Ok(job)
            }
        }
    }

    fn complete_succeeded_prediction(
        &self,
        service: &MediaGenerationService,
        job: MediaJob,
        prediction: &ReplicatePrediction,
        now_ms: u64,
    ) -> Result<MediaJob> {
        let output_url = prediction.output_url()?;
        complete_video_job(
            service,
            job,
            CompletedVideoTask {
                provider_id: REPLICATE_PROVIDER_ID,
                task_id: &prediction.id,
                remote_status: &prediction.status,
                video_url: Some(output_url.as_str()),
                filename_prefix: "replicate-video",
                missing_url_message: "completed Replicate prediction is missing output URL",
            },
            now_ms,
            |url| self.transport.download_bytes(url),
        )
    }
}

#[derive(Debug, Clone)]
struct ReplicatePrediction {
    id: String,
    status: String,
    get_url: Option<String>,
    output: Option<Value>,
    error: Option<String>,
}

impl ReplicatePrediction {
    fn from_value(value: Value) -> Result<Self> {
        let id = value
            .get("id")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("Replicate prediction response missing id")?
            .to_string();
        let status = value
            .get("status")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .context("Replicate prediction response missing status")?
            .to_string();
        let urls = value.get("urls");
        Ok(Self {
            id,
            status,
            get_url: urls
                .and_then(|urls| urls.get("get"))
                .and_then(Value::as_str)
                .map(str::to_string),
            output: value.get("output").cloned(),
            error: value
                .get("error")
                .and_then(Value::as_str)
                .map(str::to_string),
        })
    }

    fn media_status(&self) -> Result<MediaJobStatus> {
        match self.status.trim().to_ascii_lowercase().as_str() {
            "starting" => Ok(MediaJobStatus::Queued),
            "processing" => Ok(MediaJobStatus::Running),
            "succeeded" => Ok(MediaJobStatus::Succeeded),
            "failed" => Ok(MediaJobStatus::Failed),
            "canceled" | "cancelled" => Ok(MediaJobStatus::Canceled),
            other => bail!("unknown Replicate prediction status `{other}`"),
        }
    }

    fn output_url(&self) -> Result<String> {
        match self.output.as_ref() {
            Some(Value::String(url)) if !url.trim().is_empty() => Ok(url.trim().to_string()),
            Some(Value::Array(items)) => items
                .iter()
                .find_map(Value::as_str)
                .map(str::trim)
                .filter(|url| !url.is_empty())
                .map(str::to_string)
                .context("Replicate video prediction output is empty"),
            _ => bail!("Replicate video prediction is missing output URL"),
        }
    }
}

fn apply_remote_prediction_metadata(job: &mut MediaJob, prediction: &ReplicatePrediction) {
    job.provider_job_id = Some(prediction.id.clone());
    job.remote_status = Some(prediction.status.clone());
    if let Some(get_url) = &prediction.get_url {
        job.remote_get_url = Some(get_url.clone());
    }
}

fn json_response(response: reqwest::blocking::Response, context: &str) -> Result<Value> {
    let status = response.status();
    let body = response
        .text()
        .with_context(|| format!("read {context} response"))?;
    if !status.is_success() {
        bail!("{context} failed with status {}: {body}", status.as_u16());
    }
    serde_json::from_str(&body).with_context(|| format!("parse {context} response"))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::media::{MediaGenerationService, MediaJobStatus};
    use anyhow::{anyhow, Context, Result};
    use serde_json::{json, Value};
    use std::collections::{HashMap, VecDeque};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    #[derive(Clone, Default)]
    struct FakeReplicateTransport {
        state: Arc<Mutex<FakeReplicateState>>,
    }

    #[derive(Default)]
    struct FakeReplicateState {
        submit_body: Option<Value>,
        submit_response: Value,
        poll_responses: VecDeque<Value>,
        downloads: HashMap<String, std::result::Result<Vec<u8>, String>>,
    }

    impl FakeReplicateTransport {
        fn with_submit_response(response: Value) -> Self {
            let transport = Self::default();
            transport.state.lock().unwrap().submit_response = response;
            transport
        }

        fn push_poll_response(&self, response: Value) {
            self.state
                .lock()
                .unwrap()
                .poll_responses
                .push_back(response);
        }

        fn set_download(&self, url: &str, bytes: std::result::Result<Vec<u8>, String>) {
            self.state
                .lock()
                .unwrap()
                .downloads
                .insert(url.to_string(), bytes);
        }

        fn submit_body(&self) -> Value {
            self.state.lock().unwrap().submit_body.clone().unwrap()
        }
    }

    impl ReplicateVideoTransport for FakeReplicateTransport {
        fn submit_prediction(
            &self,
            _base_url: &str,
            _api_token: &str,
            body: &Value,
        ) -> Result<Value> {
            let mut state = self.state.lock().unwrap();
            state.submit_body = Some(body.clone());
            Ok(state.submit_response.clone())
        }

        fn poll_prediction(&self, _url: &str, _api_token: &str) -> Result<Value> {
            self.state
                .lock()
                .unwrap()
                .poll_responses
                .pop_front()
                .context("missing fake poll response")
        }

        fn download_bytes(&self, url: &str) -> Result<Vec<u8>> {
            match self.state.lock().unwrap().downloads.remove(url) {
                Some(Ok(bytes)) => Ok(bytes),
                Some(Err(error)) => Err(anyhow!(error)),
                None => Err(anyhow!("missing fake download for {url}")),
            }
        }
    }

    fn request() -> ReplicateVideoRequest {
        ReplicateVideoRequest {
            model: "owner/video-model:version-1".to_string(),
            prompt: "animate a product logo".to_string(),
            aspect_ratio: "16:9".to_string(),
            duration_seconds: 8,
        }
    }

    fn submit_response(status: &str) -> Value {
        json!({
            "id": "prediction-1",
            "status": status,
            "urls": {
                "get": "https://api.replicate.test/v1/predictions/prediction-1",
                "cancel": "https://api.replicate.test/v1/predictions/prediction-1/cancel"
            }
        })
    }

    #[test]
    fn replicate_video_submit_saves_queued_prediction_job() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(submit_response("starting"));
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport.clone(),
        );

        let job = adapter.submit(&service, request(), 10).unwrap();

        assert_eq!(job.status, MediaJobStatus::Queued);
        assert_eq!(job.provider_id, "replicate");
        assert_eq!(job.model_id, "owner/video-model:version-1");
        assert_eq!(job.provider_job_id.as_deref(), Some("prediction-1"));
        assert_eq!(
            service.load_job(&job.id).unwrap().status,
            MediaJobStatus::Queued
        );
        let body = transport.submit_body();
        assert_eq!(body["version"], "owner/video-model:version-1");
        assert_eq!(body["input"]["prompt"], "animate a product logo");
        assert_eq!(body["input"]["aspect_ratio"], "16:9");
        assert_eq!(body["input"]["duration"], 8);
    }

    #[test]
    fn replicate_video_poll_downloads_completed_video_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(submit_response("starting"));
        transport.push_poll_response(json!({
            "id": "prediction-1",
            "status": "succeeded",
            "output": "https://replicate.delivery/video.mp4",
            "urls": {
                "get": "https://api.replicate.test/v1/predictions/prediction-1",
                "cancel": "https://api.replicate.test/v1/predictions/prediction-1/cancel"
            }
        }));
        transport.set_download(
            "https://replicate.delivery/video.mp4",
            Ok(b"mp4-bytes".to_vec()),
        );
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport,
        );
        let job = adapter.submit(&service, request(), 10).unwrap();

        let completed = adapter.poll(&service, job, 11).unwrap();

        assert_eq!(completed.status, MediaJobStatus::Succeeded);
        assert_eq!(completed.artifact_ids.len(), 1);
        let artifact = service.load_artifact(&completed.artifact_ids[0]).unwrap();
        assert_eq!(artifact.mime_type, "video/mp4");
        assert!(artifact
            .path
            .starts_with(temp.path().join(".puffer/media/videos")));
        assert_eq!(std::fs::read(&artifact.path).unwrap(), b"mp4-bytes");
    }

    #[test]
    fn replicate_video_submit_downloads_immediately_succeeded_prediction_artifact() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(json!({
            "id": "prediction-1",
            "status": "succeeded",
            "output": "https://replicate.delivery/video.mp4",
            "urls": {
                "get": "https://api.replicate.test/v1/predictions/prediction-1",
                "cancel": "https://api.replicate.test/v1/predictions/prediction-1/cancel"
            }
        }));
        transport.set_download(
            "https://replicate.delivery/video.mp4",
            Ok(b"mp4-bytes".to_vec()),
        );
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport,
        );

        let completed = adapter.submit(&service, request(), 10).unwrap();

        assert_eq!(completed.status, MediaJobStatus::Succeeded);
        assert_eq!(completed.artifact_ids.len(), 1);
        let artifact = service.load_artifact(&completed.artifact_ids[0]).unwrap();
        assert_eq!(artifact.mime_type, "video/mp4");
        assert!(artifact
            .path
            .starts_with(temp.path().join(".puffer/media/videos")));
        assert_eq!(std::fs::read(&artifact.path).unwrap(), b"mp4-bytes");
    }

    #[test]
    fn replicate_video_poll_marks_failed_provider_status_terminal() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(submit_response("starting"));
        transport.push_poll_response(json!({
            "id": "prediction-1",
            "status": "failed",
            "error": "provider render failed"
        }));
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport,
        );
        let job = adapter.submit(&service, request(), 10).unwrap();

        let failed = adapter.poll(&service, job, 11).unwrap();

        assert_eq!(failed.status, MediaJobStatus::Failed);
        assert_eq!(failed.error.as_deref(), Some("provider render failed"));
    }

    #[test]
    fn replicate_video_poll_until_terminal_uses_bounded_backoff_for_retryable_status() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(submit_response("starting"));
        transport.push_poll_response(submit_response("processing"));
        transport.push_poll_response(json!({
            "id": "prediction-1",
            "status": "succeeded",
            "output": ["https://replicate.delivery/video.mp4"]
        }));
        transport.set_download(
            "https://replicate.delivery/video.mp4",
            Ok(b"mp4-bytes".to_vec()),
        );
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport,
        );
        let job = adapter.submit(&service, request(), 10).unwrap();
        let mut sleeps = Vec::new();

        let completed = adapter
            .poll_until_terminal_with_sleep(
                &service,
                job,
                ReplicatePollingConfig::new(3, 5, 20),
                || 11,
                |duration| sleeps.push(duration),
            )
            .unwrap();

        assert_eq!(completed.status, MediaJobStatus::Succeeded);
        assert_eq!(sleeps, vec![Duration::from_millis(5)]);
    }

    #[test]
    fn replicate_video_expired_result_url_marks_job_failed() {
        let temp = tempfile::tempdir().unwrap();
        let service = MediaGenerationService::new(temp.path());
        let transport = FakeReplicateTransport::with_submit_response(submit_response("starting"));
        transport.push_poll_response(json!({
            "id": "prediction-1",
            "status": "succeeded",
            "output": "https://replicate.delivery/expired.mp4"
        }));
        transport.set_download(
            "https://replicate.delivery/expired.mp4",
            Err("remote URL expired".to_string()),
        );
        let adapter = ReplicateVideoAdapter::with_transport(
            "token-1",
            "https://api.replicate.test",
            transport,
        );
        let job = adapter.submit(&service, request(), 10).unwrap();

        let error = adapter.poll(&service, job.clone(), 11).unwrap_err();
        let saved = service.load_job(&job.id).unwrap();

        assert!(error.to_string().contains("remote URL expired"));
        assert_eq!(saved.status, MediaJobStatus::Failed);
        assert!(saved.error.unwrap().contains("remote URL expired"));
    }
}
