use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::Duration;
use url::Url;

const ENABLED_ENV: &str = "PUFFER_HEARTBEAT_ENABLED";
const URL_ENV: &str = "PUFFER_HEARTBEAT_URL";
const INTERVAL_SECONDS_ENV: &str = "PUFFER_HEARTBEAT_INTERVAL_SECONDS";

const DEFAULT_INTERVAL_SECONDS: u64 = 60;
const MIN_INTERVAL_SECONDS: u64 = 5;
const REQUEST_TIMEOUT_SECONDS: u64 = 10;
const IDLE_POLL_SECONDS: u64 = 1;

/// Handle for the heartbeat worker.
pub(crate) struct HeartbeatHandle {
    stop: Arc<StopSignal>,
    thread: Option<thread::JoinHandle<()>>,
}

impl HeartbeatHandle {
    fn start(config: HeartbeatConfig) -> Result<Self> {
        let client = Client::builder()
            .timeout(Duration::from_secs(REQUEST_TIMEOUT_SECONDS))
            .build()
            .context("build heartbeat HTTP client")?;
        let stop = Arc::new(StopSignal::default());
        let thread_stop = Arc::clone(&stop);
        let thread = thread::Builder::new()
            .name("puffer-heartbeat".to_string())
            .spawn(move || run_heartbeat_loop(client, config, thread_stop))
            .context("spawn heartbeat thread")?;
        Ok(Self {
            stop,
            thread: Some(thread),
        })
    }
}

impl Drop for HeartbeatHandle {
    fn drop(&mut self) {
        self.stop.stop();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// Starts the heartbeat worker when environment config enables it.
pub(crate) fn start_from_env() -> Result<Option<HeartbeatHandle>> {
    let Some(config) = HeartbeatConfig::from_env()? else {
        return Ok(None);
    };
    HeartbeatHandle::start(config).map(Some)
}

#[derive(Clone, Debug)]
struct HeartbeatConfig {
    url: Url,
    interval: Duration,
    secret_values: Vec<String>,
}

impl HeartbeatConfig {
    fn from_env() -> Result<Option<Self>> {
        resolve_config(
            trimmed_env(ENABLED_ENV).as_deref(),
            trimmed_env(URL_ENV).as_deref(),
            trimmed_env(INTERVAL_SECONDS_ENV).as_deref(),
        )
    }
}

#[derive(Default)]
struct StopSignal {
    stopped: Mutex<bool>,
    changed: Condvar,
}

impl StopSignal {
    fn stop(&self) {
        let mut stopped = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        *stopped = true;
        self.changed.notify_all();
    }

    fn wait_or_stopped(&self, duration: Duration) -> bool {
        let stopped = self.stopped.lock().unwrap_or_else(|p| p.into_inner());
        if *stopped {
            return true;
        }
        let (stopped, _) = self
            .changed
            .wait_timeout_while(stopped, duration, |stopped| !*stopped)
            .unwrap_or_else(|p| p.into_inner());
        *stopped
    }
}

fn run_heartbeat_loop(client: Client, config: HeartbeatConfig, stop: Arc<StopSignal>) {
    let mut delay = Duration::ZERO;
    loop {
        if !puffer_core::runtime_work_active() {
            delay = Duration::ZERO;
            if stop.wait_or_stopped(Duration::from_secs(IDLE_POLL_SECONDS)) {
                break;
            }
            continue;
        }
        if !delay.is_zero() && stop.wait_or_stopped(delay) {
            break;
        }
        if stop.wait_or_stopped(Duration::ZERO) {
            break;
        }
        if !puffer_core::runtime_work_active() {
            delay = Duration::ZERO;
            continue;
        }
        if let Err(error) = send_heartbeat(&client, &config) {
            eprintln!(
                "heartbeat failed: {}",
                redact_secrets(&error.to_string(), &config.secret_values)
            );
        }
        delay = config.interval;
    }
}

fn send_heartbeat(client: &Client, config: &HeartbeatConfig) -> Result<()> {
    let response = client
        .post(config.url.clone())
        .send()
        .map_err(|error| anyhow!(error))?;
    if response.status().is_success() {
        return Ok(());
    }
    let status = response.status();
    let body = response.text().unwrap_or_default();
    let body = body.trim();
    if body.is_empty() {
        bail!("HTTP {status}");
    }
    bail!("HTTP {status}: {body}");
}

fn resolve_config(
    enabled_value: Option<&str>,
    url_value: Option<&str>,
    interval_value: Option<&str>,
) -> Result<Option<HeartbeatConfig>> {
    let enabled = parse_optional_bool(enabled_value)?;
    let Some(raw_url) = url_value.filter(|value| !value.trim().is_empty()) else {
        if enabled == Some(true) {
            bail!("{ENABLED_ENV}=true requires {URL_ENV}");
        }
        return Ok(None);
    };
    if enabled == Some(false) {
        return Ok(None);
    }

    let interval_seconds = parse_seconds(interval_value, DEFAULT_INTERVAL_SECONDS)
        .max(MIN_INTERVAL_SECONDS)
        .min(300);

    let url = Url::parse(raw_url).with_context(|| format!("parse {URL_ENV}"))?;
    let mut secret_values = Vec::new();
    for token in url
        .query_pairs()
        .filter_map(|(name, value)| (name == "token").then(|| value.into_owned()))
        .filter(|value| !value.is_empty())
    {
        push_secret_variants(&mut secret_values, &token);
    }

    Ok(Some(HeartbeatConfig {
        url,
        interval: Duration::from_secs(interval_seconds),
        secret_values,
    }))
}

fn parse_optional_bool(value: Option<&str>) -> Result<Option<bool>> {
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(None);
    };
    if value == "1" || value.eq_ignore_ascii_case("true") {
        return Ok(Some(true));
    }
    if value == "0" || value.eq_ignore_ascii_case("false") {
        return Ok(Some(false));
    }
    bail!("{ENABLED_ENV} must be one of 1, true, 0, or false")
}

fn parse_seconds(value: Option<&str>, default: u64) -> u64 {
    value
        .and_then(|value| value.trim().parse::<u64>().ok())
        .unwrap_or(default)
}

fn trimmed_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn redact_secrets(message: &str, secrets: &[String]) -> String {
    secrets.iter().fold(message.to_string(), |message, secret| {
        if secret.is_empty() {
            message
        } else {
            message.replace(secret, "[redacted]")
        }
    })
}

fn push_secret_variants(secrets: &mut Vec<String>, secret: &str) {
    if secret.is_empty() {
        return;
    }
    secrets.push(secret.to_string());
    let encoded = url::form_urlencoded::byte_serialize(secret.as_bytes()).collect::<String>();
    if encoded != secret {
        secrets.push(encoded);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_is_disabled_without_url() {
        let config = resolve_config(None, None, None).unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn config_requires_url_when_enabled() {
        let error = resolve_config(Some("true"), None, None).expect_err("missing URL should fail");
        assert!(error.to_string().contains(URL_ENV));
    }

    #[test]
    fn config_accepts_url_without_token_query() {
        let config = resolve_config(None, Some("https://api.example.test/v1/heartbeat"), None)
            .unwrap()
            .expect("config");

        assert_eq!(config.url.as_str(), "https://api.example.test/v1/heartbeat");
        assert_eq!(
            config.interval,
            Duration::from_secs(DEFAULT_INTERVAL_SECONDS)
        );
    }

    #[test]
    fn config_accepts_token_already_in_url() {
        let config = resolve_config(
            Some("1"),
            Some("https://api.example.test/v1/heartbeat?token=url-token"),
            Some("20"),
        )
        .unwrap()
        .expect("config");

        assert!(config.secret_values.contains(&"url-token".to_string()));
        assert_eq!(config.interval, Duration::from_secs(20));
    }

    #[test]
    fn config_disabled_flag_overrides_url() {
        let config = resolve_config(
            Some("false"),
            Some("https://api.example.test/v1/heartbeat?token=url-token"),
            None,
        )
        .unwrap();
        assert!(config.is_none());
    }

    #[test]
    fn redact_secrets_removes_token_values_from_messages() {
        let message = "request failed for https://x.test/heartbeat?token=secret-token";
        assert_eq!(
            redact_secrets(message, &["secret-token".to_string()]),
            "request failed for https://x.test/heartbeat?token=[redacted]"
        );
    }

    #[test]
    fn redact_secrets_removes_url_encoded_token_values() {
        let mut secrets = Vec::new();
        push_secret_variants(&mut secrets, "secret token");
        let message = "request failed for https://x.test/heartbeat?token=secret+token";
        assert_eq!(
            redact_secrets(message, &secrets),
            "request failed for https://x.test/heartbeat?token=[redacted]"
        );
    }
}
