use crate::events::EventEmitter;
use anyhow::{anyhow, bail, Context, Result};
use reqwest::blocking::Client;
use serde::Serialize;
use serde_json::{json, Value};
use std::env;
use std::fs::{self, OpenOptions};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

const QWEN35_ID: &str = "qwen35";
const QWEN35_MODEL_ID: &str = "qwen3.5-0.8b";
const QWEN35_DISPLAY_NAME: &str = "Qwen3.5-0.8B (local)";
const QWEN35_REPO: &str = "mlx-community/Qwen3.5-0.8B-8bit";
const QWEN35_ENDPOINT: &str = "http://127.0.0.1:8088/v1";
const QWEN35_MODELS_URL: &str = "http://127.0.0.1:8088/v1/models";
const QWEN35_EVENT: &str = "local-model:qwen35:event";
const QWEN35_SIZE: &str = "~992MB";
const QWEN35_SHIM: &str = include_str!("../../../../scripts/qwen35_shim.py");
const QWEN35_PROVIDER: &str = include_str!("../../../../resources/providers/qwen35.yaml");

#[derive(Clone)]
pub(crate) struct LocalModelInstaller {
    installing: Arc<AtomicBool>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalModelStatus {
    id: String,
    model_id: String,
    display_name: String,
    checked_at_ms: u64,
    supported: bool,
    recommended: bool,
    installed: bool,
    configured: bool,
    running: bool,
    installing: bool,
    reason: String,
    endpoint: String,
    size: String,
    install_path: String,
    provider_path: String,
    log_path: String,
    install_log_path: String,
    serve_log_path: String,
    checks: Vec<LocalModelCheck>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalModelCheck {
    label: String,
    state: String,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LocalModelInstallJob {
    job_id: String,
    status: LocalModelStatus,
}

struct Qwen35Paths {
    puffer_home: PathBuf,
    venv: PathBuf,
    python: PathBuf,
    model_dir: PathBuf,
    model_config: PathBuf,
    bin_dir: PathBuf,
    shim: PathBuf,
    provider: PathBuf,
    install_log: PathBuf,
    serve_log: PathBuf,
}

struct Qwen35Health {
    running: bool,
    detail: String,
}

impl LocalModelInstaller {
    /// Creates a local-model installer with in-process job de-duplication.
    pub(crate) fn new() -> Self {
        Self {
            installing: Arc::new(AtomicBool::new(false)),
        }
    }

    /// Returns the current installation and server status for a local model.
    pub(crate) fn status(&self, model_id: &str) -> Result<LocalModelStatus> {
        ensure_qwen35(model_id)?;
        Ok(build_status(self.installing.load(Ordering::SeqCst)))
    }

    /// Starts the one-click Qwen3.5 installer or returns the active job state.
    pub(crate) fn install_or_start(
        &self,
        events: EventEmitter,
        model_id: &str,
    ) -> Result<LocalModelInstallJob> {
        ensure_qwen35(model_id)?;
        if self
            .installing
            .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
            .is_err()
        {
            return Ok(LocalModelInstallJob {
                job_id: "active".to_string(),
                status: build_status(true),
            });
        }

        let job_id = Uuid::new_v4().to_string();
        let installer = self.clone();
        let thread_job_id = job_id.clone();
        thread::spawn(move || {
            let result = installer.run_install_job(&events, &thread_job_id);
            if let Err(error) = result {
                emit_model_event(
                    &events,
                    &thread_job_id,
                    "error",
                    format!("{error:#}"),
                    Some(build_status(false)),
                );
            }
            installer.installing.store(false, Ordering::SeqCst);
        });

        Ok(LocalModelInstallJob {
            job_id,
            status: build_status(true),
        })
    }

    fn run_install_job(&self, events: &EventEmitter, job_id: &str) -> Result<()> {
        let paths = qwen35_paths();
        let status = build_status(true);
        if !status.supported {
            bail!(status.reason);
        }

        fs::create_dir_all(&paths.puffer_home).with_context(|| {
            format!(
                "failed to create Puffer home {}",
                paths.puffer_home.display()
            )
        })?;
        reset_log(&paths.install_log)?;
        emit_model_event(
            events,
            job_id,
            "checking",
            "Checking local Qwen3.5 runtime".to_string(),
            Some(status),
        );

        if !paths.python.exists() {
            emit_model_event(
                events,
                job_id,
                "runtime",
                format!("Creating isolated Python venv at {}", paths.venv.display()),
                None,
            );
            run_logged(
                "runtime",
                "python3",
                &["-m", "venv", path_str(&paths.venv)?],
                &[],
                &paths.install_log,
            )?;
        }

        if !python_imports(&paths.python, "mlx_lm")
            || !python_imports(&paths.python, "huggingface_hub")
        {
            emit_model_event(
                events,
                job_id,
                "runtime",
                "Installing mlx-lm and huggingface_hub into the isolated venv".to_string(),
                None,
            );
            run_logged(
                "pip-upgrade",
                path_str(&paths.python)?,
                &["-m", "pip", "install", "--quiet", "--upgrade", "pip"],
                &[],
                &paths.install_log,
            )?;
            run_logged(
                "pip-install",
                path_str(&paths.python)?,
                &[
                    "-m",
                    "pip",
                    "install",
                    "--quiet",
                    "--upgrade",
                    "mlx-lm",
                    "huggingface_hub",
                ],
                &[],
                &paths.install_log,
            )?;
        }

        if !paths.model_config.exists() {
            emit_model_event(
                events,
                job_id,
                "download",
                format!("Downloading {QWEN35_REPO} ({QWEN35_SIZE})"),
                None,
            );
            fs::create_dir_all(&paths.model_dir).with_context(|| {
                format!("failed to create model dir {}", paths.model_dir.display())
            })?;
            run_logged(
                "download",
                path_str(&paths.python)?,
                &[
                    "-c",
                    "import os; from huggingface_hub import snapshot_download; snapshot_download(os.environ['QWEN35_REPO'], local_dir=os.environ['QWEN35_MODEL_DIR'])",
                ],
                &[
                    ("QWEN35_REPO", QWEN35_REPO.to_string()),
                    (
                        "QWEN35_MODEL_DIR",
                        paths.model_dir.display().to_string(),
                    ),
                ],
                &paths.install_log,
            )?;
        } else {
            emit_model_event(
                events,
                job_id,
                "download",
                "Model weights already present".to_string(),
                None,
            );
        }

        emit_model_event(
            events,
            job_id,
            "configure",
            "Installing shim and registering the Puffer provider".to_string(),
            None,
        );
        fs::create_dir_all(&paths.bin_dir)
            .with_context(|| format!("failed to create {}", paths.bin_dir.display()))?;
        fs::create_dir_all(paths.provider.parent().unwrap_or(&paths.puffer_home))?;
        fs::write(&paths.shim, QWEN35_SHIM)
            .with_context(|| format!("failed to write {}", paths.shim.display()))?;
        fs::write(&paths.provider, QWEN35_PROVIDER)
            .with_context(|| format!("failed to write {}", paths.provider.display()))?;

        emit_model_event(
            events,
            job_id,
            "serve",
            format!("Starting local server at {QWEN35_ENDPOINT}"),
            None,
        );
        start_qwen35_server(&paths)?;
        wait_for_qwen35_ready()?;

        emit_model_event(
            events,
            job_id,
            "done",
            "Qwen3.5 is installed, registered, and running".to_string(),
            Some(build_status(false)),
        );
        Ok(())
    }
}

fn ensure_qwen35(model_id: &str) -> Result<()> {
    let normalized = model_id.trim();
    if normalized.is_empty() || normalized == QWEN35_ID || normalized == QWEN35_MODEL_ID {
        return Ok(());
    }
    bail!("unsupported local model `{normalized}`")
}

fn build_status(installing: bool) -> LocalModelStatus {
    let paths = qwen35_paths();
    let supported = supports_qwen35();
    let python_exists = paths.python.exists();
    let mlx_installed = python_exists && python_imports(&paths.python, "mlx_lm");
    let hub_installed = python_exists && python_imports(&paths.python, "huggingface_hub");
    let deps_installed = mlx_installed && hub_installed;
    let model_present = paths.model_config.exists();
    let shim_present = paths.shim.exists();
    let installed = model_present && python_exists && deps_installed && shim_present;
    let configured = paths.provider.exists();
    let health = qwen35_health();
    let running = health.running;
    let recommended = supported && !installed;
    let reason = if !supported {
        unsupported_reason()
    } else if installing {
        "installing".to_string()
    } else if installed && configured && running {
        "ready".to_string()
    } else if installed && configured {
        "installed; server is stopped".to_string()
    } else if installed {
        "installed; provider registration missing".to_string()
    } else {
        "macOS Apple Silicon, model not yet installed".to_string()
    };
    let checks = build_checks(
        &paths,
        supported,
        python_exists,
        mlx_installed,
        hub_installed,
        model_present,
        shim_present,
        configured,
        &health,
    );

    LocalModelStatus {
        id: QWEN35_ID.to_string(),
        model_id: QWEN35_MODEL_ID.to_string(),
        display_name: QWEN35_DISPLAY_NAME.to_string(),
        checked_at_ms: now_ms(),
        supported,
        recommended,
        installed,
        configured,
        running,
        installing,
        reason,
        endpoint: QWEN35_ENDPOINT.to_string(),
        size: QWEN35_SIZE.to_string(),
        install_path: paths.model_dir.display().to_string(),
        provider_path: paths.provider.display().to_string(),
        log_path: paths.serve_log.display().to_string(),
        install_log_path: paths.install_log.display().to_string(),
        serve_log_path: paths.serve_log.display().to_string(),
        checks,
    }
}

#[allow(clippy::too_many_arguments)]
fn build_checks(
    paths: &Qwen35Paths,
    supported: bool,
    python_exists: bool,
    mlx_installed: bool,
    hub_installed: bool,
    model_present: bool,
    shim_present: bool,
    configured: bool,
    health: &Qwen35Health,
) -> Vec<LocalModelCheck> {
    let mut checks = Vec::new();
    checks.push(check(
        "Platform",
        if supported { "ok" } else { "error" },
        if supported {
            format!(
                "{} {} supports Qwen3.5 MLX",
                env::consts::OS,
                env::consts::ARCH
            )
        } else {
            unsupported_reason()
        },
    ));
    checks.push(check(
        "Python venv",
        if python_exists { "ok" } else { "missing" },
        if python_exists {
            format!("found {}", paths.python.display())
        } else {
            format!("missing {}", paths.python.display())
        },
    ));
    checks.push(check(
        "Python deps",
        if mlx_installed && hub_installed {
            "ok"
        } else {
            "missing"
        },
        dependency_detail(python_exists, mlx_installed, hub_installed),
    ));
    checks.push(check(
        "Model weights",
        if model_present { "ok" } else { "missing" },
        if model_present {
            format!("config.json present in {}", paths.model_dir.display())
        } else {
            format!("missing {}", paths.model_config.display())
        },
    ));
    checks.push(check(
        "Shim",
        if shim_present { "ok" } else { "missing" },
        file_check_detail(&paths.shim, "shim script"),
    ));
    checks.push(check(
        "Provider YAML",
        if configured { "ok" } else { "missing" },
        file_check_detail(&paths.provider, "provider registration"),
    ));
    checks.push(check(
        "Server health",
        if health.running { "ok" } else { "warning" },
        health.detail.clone(),
    ));
    checks
}

fn check(label: &str, state: &str, detail: String) -> LocalModelCheck {
    LocalModelCheck {
        label: label.to_string(),
        state: state.to_string(),
        detail,
    }
}

fn dependency_detail(python_exists: bool, mlx_installed: bool, hub_installed: bool) -> String {
    if !python_exists {
        return "venv is missing; installer will create it".to_string();
    }
    if mlx_installed && hub_installed {
        return "mlx_lm and huggingface_hub import successfully".to_string();
    }
    let mut missing = Vec::new();
    if !mlx_installed {
        missing.push("mlx_lm");
    }
    if !hub_installed {
        missing.push("huggingface_hub");
    }
    format!("missing imports: {}", missing.join(", "))
}

fn file_check_detail(path: &PathBuf, label: &str) -> String {
    match fs::metadata(path) {
        Ok(metadata) => format!(
            "{label} present at {} ({} bytes)",
            path.display(),
            metadata.len()
        ),
        Err(_) => format!("{label} missing at {}", path.display()),
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or_default()
}

fn qwen35_paths() -> Qwen35Paths {
    let puffer_home = env::var_os("PUFFER_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home_dir().join(".puffer"));
    let venv = puffer_home.join("venvs/qwen35");
    let python = venv.join("bin/python");
    let model_dir = puffer_home.join("models/qwen3.5-0.8b");
    let model_config = model_dir.join("config.json");
    let bin_dir = puffer_home.join("bin");
    let shim = bin_dir.join("qwen35-shim.py");
    let provider = puffer_home.join("resources/providers/qwen35.yaml");
    let install_log = puffer_home.join("qwen35-install.log");
    let serve_log = puffer_home.join("qwen35-serve.log");
    Qwen35Paths {
        puffer_home,
        venv,
        python,
        model_dir,
        model_config,
        bin_dir,
        shim,
        provider,
        install_log,
        serve_log,
    }
}

fn supports_qwen35() -> bool {
    env::consts::OS == "macos" && matches!(env::consts::ARCH, "aarch64" | "arm64")
}

fn unsupported_reason() -> String {
    if env::consts::OS != "macos" {
        return format!(
            "Qwen3.5 MLX install requires macOS; detected {}",
            env::consts::OS
        );
    }
    format!(
        "Qwen3.5 MLX install requires Apple Silicon; detected {}",
        env::consts::ARCH
    )
}

fn home_dir() -> PathBuf {
    env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| env::current_dir().unwrap_or_else(|_| PathBuf::from(".")))
}

fn path_str(path: &PathBuf) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path is not valid UTF-8: {}", path.display()))
}

fn python_imports(python: &PathBuf, module: &str) -> bool {
    Command::new(python)
        .arg("-c")
        .arg(format!("import {module}"))
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn reset_log(path: &PathBuf) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    fs::write(path, "")?;
    Ok(())
}

fn append_log(path: &PathBuf, text: &str) -> Result<()> {
    use std::io::Write;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut file = OpenOptions::new().create(true).append(true).open(path)?;
    file.write_all(text.as_bytes())?;
    Ok(())
}

fn run_logged(
    phase: &str,
    program: &str,
    args: &[&str],
    envs: &[(&str, String)],
    log_path: &PathBuf,
) -> Result<()> {
    append_log(log_path, &format!("\n$ {program} {}\n", args.join(" ")))?;
    let mut command = Command::new(program);
    command.args(args);
    for (key, value) in envs {
        command.env(key, value);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to run {program}"))?;
    append_log(log_path, &String::from_utf8_lossy(&output.stdout))?;
    append_log(log_path, &String::from_utf8_lossy(&output.stderr))?;
    if output.status.success() {
        return Ok(());
    }
    bail!(
        "{phase} command failed with status {}. See {}",
        output.status,
        log_path.display()
    )
}

fn start_qwen35_server(paths: &Qwen35Paths) -> Result<()> {
    if qwen35_running() {
        return Ok(());
    }
    if paths.serve_log.exists() {
        let _ = fs::remove_file(&paths.serve_log);
    }
    if let Some(parent) = paths.serve_log.parent() {
        fs::create_dir_all(parent)?;
    }
    let stdout = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&paths.serve_log)?;
    let stderr = stdout.try_clone()?;
    Command::new(&paths.python)
        .arg(&paths.shim)
        .env("QWEN35_MODEL", &paths.model_dir)
        .stdout(Stdio::from(stdout))
        .stderr(Stdio::from(stderr))
        .spawn()
        .with_context(|| format!("failed to start {}", paths.shim.display()))?;
    Ok(())
}

fn wait_for_qwen35_ready() -> Result<()> {
    for _ in 0..90 {
        if qwen35_running() {
            return Ok(());
        }
        thread::sleep(Duration::from_secs(1));
    }
    bail!("Qwen3.5 server did not become ready at {QWEN35_ENDPOINT}")
}

fn qwen35_running() -> bool {
    qwen35_health().running
}

fn qwen35_health() -> Qwen35Health {
    let client = match Client::builder().timeout(Duration::from_secs(2)).build() {
        Ok(client) => client,
        Err(error) => {
            return Qwen35Health {
                running: false,
                detail: format!("failed to build health client: {error}"),
            };
        }
    };
    let response = match client.get(QWEN35_MODELS_URL).send() {
        Ok(response) => response,
        Err(error) => {
            return Qwen35Health {
                running: false,
                detail: format!("{QWEN35_MODELS_URL} is not reachable: {error}"),
            };
        }
    };
    let status = response.status();
    let body = response.text().unwrap_or_default();
    if !status.is_success() {
        return Qwen35Health {
            running: false,
            detail: format!("{QWEN35_MODELS_URL} returned HTTP {status}"),
        };
    }
    let model_advertised = serde_json::from_str::<Value>(&body)
        .ok()
        .and_then(|value| value.get("data").and_then(Value::as_array).cloned())
        .map(|models| {
            models.iter().any(|model| {
                model
                    .get("id")
                    .and_then(Value::as_str)
                    .map(|id| id == QWEN35_MODEL_ID)
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false);
    if model_advertised {
        return Qwen35Health {
            running: true,
            detail: format!("{QWEN35_MODELS_URL} advertises {QWEN35_MODEL_ID}"),
        };
    }
    Qwen35Health {
        running: false,
        detail: format!("{QWEN35_MODELS_URL} answered but did not advertise {QWEN35_MODEL_ID}"),
    }
}

fn emit_model_event(
    events: &EventEmitter,
    job_id: &str,
    phase: &str,
    message: String,
    status: Option<LocalModelStatus>,
) {
    events.emit(
        QWEN35_EVENT,
        json!({
            "modelId": QWEN35_MODEL_ID,
            "jobId": job_id,
            "phase": phase,
            "message": message,
            "status": status,
        }),
    );
}
