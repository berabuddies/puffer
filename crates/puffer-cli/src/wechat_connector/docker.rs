//! Docker lifecycle and `exec` helpers for a managed WeChat desktop instance.
//!
//! A WeChat "instance" is one container named `puffer-wechat-<name>`, based on
//! the WechatOnCloud image (`ghcr.io/gloridust/wechat-on-cloud`), which bundles
//! Xvfb + openbox + KasmVNC + the native WeChat client plus `xdotool`/`xclip`.
//! Puffer talks to it exclusively through the `docker` CLI (matching how the
//! browser worker shells out to Chrome), so there is no new dependency.
//!
//! All input/read commands run as the in-container `abc` user (the user WeChat
//! runs as) and export `DISPLAY` by probing `/tmp/.X11-unix` the same way the
//! WechatOnCloud panel does.

use anyhow::{bail, Context, Result};
use std::process::Stdio;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::config::InstanceConfig;

/// Default WeChat runtime image (WechatOnCloud, multi-arch amd64/arm64).
pub(crate) const DEFAULT_IMAGE: &str = "ghcr.io/gloridust/wechat-on-cloud:latest";

/// Shell prelude that resolves `DISPLAY` inside the container exactly like the
/// WechatOnCloud panel: honor an existing `$DISPLAY`, else take the first
/// `/tmp/.X11-unix/X*` socket, else fall back to `:1`.
const DISPLAY_PRELUDE: &str = concat!(
    "display=\"${DISPLAY:-}\"; ",
    "if [ -z \"$display\" ]; then ",
    "for x in /tmp/.X11-unix/X*; do [ -e \"$x\" ] || continue; display=\":${x##*X}\"; break; done; ",
    "fi; ",
    "export DISPLAY=\"${display:-:1}\"; "
);

/// Container runtime backend. Docker is the default and the only path validated
/// end-to-end; Apple `container` (macOS, built into the OS — no Docker Desktop
/// install) is supported as a near drop-in (its `run`/`exec` CLI is
/// flag-compatible), selected on macOS when the `container` binary is present.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Runtime {
    Docker,
    Container,
}

/// Resolves the runtime + its CLI binary. `WECHAT_RUNTIME=docker|container|auto`
/// (default `auto`). Apple `container` is currently **opt-in** (`=container`):
/// `auto` resolves to Docker so the validated path stays the default. Once the
/// `container` path is validated end-to-end, `auto` should prefer it on macOS
/// when the `container` binary is present (it only installs on macOS 26+).
/// `DOCKER_BIN` / `WECHAT_CONTAINER_BIN` override the binary path.
fn select_runtime() -> (Runtime, String) {
    match env_or("WECHAT_RUNTIME", "auto").to_ascii_lowercase().as_str() {
        "container" => (Runtime::Container, env_or("WECHAT_CONTAINER_BIN", "container")),
        // "docker" and "auto" both resolve to Docker for now.
        _ => (Runtime::Docker, env_or("DOCKER_BIN", "docker")),
    }
}

/// One managed WeChat desktop instance, addressed by container name.
#[derive(Debug, Clone)]
pub(crate) struct WechatInstance {
    /// Logical instance name (e.g. `default`); the container is `puffer-wechat-<name>`.
    name: String,
    /// CLI binary for the selected runtime (`docker` or `container`).
    docker_bin: String,
    /// Selected container runtime.
    runtime: Runtime,
}

impl WechatInstance {
    /// Builds an instance from the environment: `WECHAT_INSTANCE` (default
    /// `default`) and the runtime selection (see [`select_runtime`]).
    pub(crate) fn from_env() -> Self {
        let (runtime, docker_bin) = select_runtime();
        Self {
            name: env_or("WECHAT_INSTANCE", "default"),
            docker_bin,
            runtime,
        }
    }

    /// Builds an instance for a specific connection slug (the slug is the
    /// instance name, so each connection maps to its own container). An empty
    /// slug falls back to the env/default instance.
    pub(crate) fn for_connection(slug: &str) -> Self {
        let slug = slug.trim();
        if slug.is_empty() {
            return Self::from_env();
        }
        let (runtime, docker_bin) = select_runtime();
        Self {
            name: slug.to_string(),
            docker_bin,
            runtime,
        }
    }

    /// Returns the logical instance name.
    pub(crate) fn name(&self) -> &str {
        &self.name
    }

    /// Returns the docker container name (`puffer-wechat-<name>`).
    pub(crate) fn container_name(&self) -> String {
        format!("puffer-wechat-{}", self.name)
    }

    /// Runs `docker <args>` and returns its captured output (success or not).
    /// `kill_on_drop` so a cancelled call (e.g. the monitor racing shutdown)
    /// leaves no orphaned `docker exec`.
    async fn docker(&self, args: &[&str]) -> Result<std::process::Output> {
        Command::new(&self.docker_bin)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("run `{} {}`", self.docker_bin, args.join(" ")))
    }

    /// Owned-args variant of [`Self::docker`] for dynamically built argv.
    async fn docker_args(&self, args: &[String]) -> Result<std::process::Output> {
        Command::new(&self.docker_bin)
            .args(args)
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("run `{} {}`", self.docker_bin, args.join(" ")))
    }

    /// Reports whether the instance container exists (running or stopped).
    async fn container_exists(&self) -> Result<bool> {
        let name = self.container_name();
        Ok(self.docker(&["inspect", &name]).await?.status.success())
    }

    /// Ensures the WeChat image is present locally. Resolution order:
    /// 1. already present (`docker image inspect`),
    /// 2. build from a local context if `WECHAT_BUILD_CONTEXT` points at one
    ///    (the WechatOnCloud `docker/` dir) — used to override the default image,
    /// 3. otherwise `docker pull` (the default image is published at
    ///    `ghcr.io/gloridust/wechat-on-cloud:latest`).
    /// A clear, actionable error is returned when none succeed.
    async fn ensure_image(&self, cfg: &InstanceConfig) -> Result<()> {
        if self.runtime == Runtime::Container {
            // `container` has its own image store and no local `build` step. The
            // AT-SPI image is loaded/pulled into that store out of band, so here we
            // only check presence and give an actionable error otherwise.
            if self
                .docker(&["image", "inspect", &cfg.image])
                .await?
                .status
                .success()
            {
                return Ok(());
            }
            bail!(
                "wechat image `{}` is not in the `container` image store. Pull or load it first, \
                 e.g. `container image pull {}` (or `container image load`), then retry. The local \
                 Docker image store is separate from `container`'s.",
                cfg.image,
                cfg.image
            );
        }
        if self
            .docker(&["image", "inspect", &cfg.image])
            .await?
            .status
            .success()
        {
            return Ok(());
        }
        if let Ok(context) = std::env::var("WECHAT_BUILD_CONTEXT") {
            let context = context.trim();
            if !context.is_empty() {
                let output = self
                    .docker(&["build", "-t", &cfg.image, context])
                    .await?;
                if output.status.success() {
                    return Ok(());
                }
                bail!(
                    "failed to build wechat image `{}` from {context}: {}",
                    cfg.image,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        let output = self.docker(&["pull", &cfg.image]).await?;
        if output.status.success() {
            return Ok(());
        }
        bail!(
            "wechat image `{}` is not available: not built locally, no WECHAT_BUILD_CONTEXT \
             set to build it, and `docker pull` failed ({}). The WechatOnCloud images are not \
             published to a registry — build the image once with \
             `docker build -t {} <path-to>/WechatOnCloud/docker`, or set WECHAT_BUILD_CONTEXT \
             to that directory so puffer builds it.",
            cfg.image,
            String::from_utf8_lossy(&output.stderr).trim(),
            cfg.image
        )
    }

    /// Ensures the container is running: starts it if stopped, creates it (and
    /// pulls the image) if missing. Idempotent — safe to call before every
    /// `act`/`subscribe`. The data volume persists login across recreates.
    pub(crate) async fn ensure_container(&self, cfg: &InstanceConfig) -> Result<()> {
        // The engine itself may be off on a fresh machine — start it first.
        self.ensure_docker_daemon().await?;
        if self.is_running().await? {
            return Ok(());
        }
        let name = self.container_name();
        // If a container record exists, try to start it. Docker's `inspect` and
        // `start` can disagree (a container removed/pruned between calls, or left
        // half-removed) — so a failed start is NOT fatal: fall through to a clean
        // recreate. The named data volume survives, so login is preserved.
        if self.container_exists().await? {
            let output = self.docker(&["start", &name]).await?;
            if output.status.success() {
                return Ok(());
            }
            // Stale/inconsistent record — remove it so the recreate below is clean.
            let _ = self.docker(&["rm", "-f", &name]).await;
        }
        self.ensure_image(cfg).await?;
        self.create_container(cfg).await
    }

    /// Creates and starts a fresh container, recovering from a name conflict
    /// (a leftover container by the same name) by removing it and retrying once.
    async fn create_container(&self, cfg: &InstanceConfig) -> Result<()> {
        let args = run_args(cfg, self.runtime);
        let output = self.docker_args(&args).await?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        if stderr.contains("already in use") || stderr.contains("Conflict") {
            let _ = self.docker(&["rm", "-f", &cfg.container_name()]).await;
            let retry = self.docker_args(&args).await?;
            if retry.status.success() {
                return Ok(());
            }
            bail!(
                "failed to create wechat container `{}`: {}",
                cfg.container_name(),
                String::from_utf8_lossy(&retry.stderr).trim()
            );
        }
        bail!("failed to create wechat container `{}`: {stderr}", cfg.container_name())
    }

    /// Stops the instance container (kept, with its volume, for a later start).
    #[allow(dead_code)] // lifecycle API; used by stop/teardown paths
    pub(crate) async fn stop_container(&self) -> Result<()> {
        let name = self.container_name();
        let _ = self.docker(&["stop", "-t", "5", &name]).await?;
        Ok(())
    }

    /// Ensures the Docker engine is reachable, starting it if not so the user
    /// never has to touch a terminal. On macOS this means launching OrbStack
    /// (or Docker Desktop) and polling until the daemon answers. Override the
    /// start command with `WECHAT_DOCKER_START_CMD` (space-separated argv).
    pub(crate) async fn ensure_docker_daemon(&self) -> Result<()> {
        if self.daemon_reachable().await {
            return Ok(());
        }
        // Best-effort start: explicit override, then OrbStack, then Docker Desktop.
        let started = self.try_start_docker_engine().await;
        if !started {
            bail!(
                "Docker engine is not running and could not be started automatically. \
                 Start OrbStack (or Docker Desktop), or set WECHAT_DOCKER_START_CMD."
            );
        }
        // Poll up to ~60s for the daemon to come up.
        for _ in 0..60 {
            if self.daemon_reachable().await {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        }
        bail!("Docker engine did not become ready within 60s after start")
    }

    /// Reports whether the Docker engine answers (a cheap `docker version`).
    async fn daemon_reachable(&self) -> bool {
        self.docker(&["version", "--format", "{{.Server.Version}}"])
            .await
            .map(|output| output.status.success())
            .unwrap_or(false)
    }

    /// Attempts to launch the Docker engine; returns whether a launch command ran.
    async fn try_start_docker_engine(&self) -> bool {
        if let Ok(custom) = std::env::var("WECHAT_DOCKER_START_CMD") {
            let parts: Vec<&str> = custom.split_whitespace().collect();
            if let Some((program, args)) = parts.split_first() {
                if Command::new(program).args(args).status().await.is_ok() {
                    return true;
                }
            }
        }
        // OrbStack CLI.
        if Command::new("orb").arg("start").status().await.is_ok() {
            return true;
        }
        // macOS app launch fallback (OrbStack, then Docker Desktop).
        for app in ["OrbStack", "Docker"] {
            if Command::new("open").args(["-a", app]).status().await.map(|s| s.success()).unwrap_or(false) {
                return true;
            }
        }
        false
    }

    /// Returns the WeChat install status JSON from the in-container control
    /// script (`{phase,percent,installed,version,...}`).
    pub(crate) async fn wechat_status(&self) -> Result<serde_json::Value> {
        let raw = self.exec_bash("/woc/wechat-ctl.sh status").await?;
        serde_json::from_str(&raw).with_context(|| format!("parse wechat status: {raw}"))
    }

    /// Reports whether the WeChat client is installed inside the container.
    pub(crate) async fn wechat_installed(&self) -> Result<bool> {
        Ok(self
            .wechat_status()
            .await
            .ok()
            .and_then(|status| status.get("installed").and_then(serde_json::Value::as_bool))
            .unwrap_or(false))
    }

    /// Downloads + installs the native WeChat client inside the container
    /// (blocking until done; the official package is large). No-op if already
    /// installed. The autostart loop launches WeChat once the binary lands.
    pub(crate) async fn install_wechat(&self) -> Result<()> {
        if self.wechat_installed().await? {
            return Ok(());
        }
        // wechat-ctl.sh install is long-running; allow a generous window.
        let name = self.container_name();
        let output = Command::new(&self.docker_bin)
            .args([
                "exec",
                "--user",
                "abc",
                &name,
                "/woc/wechat-ctl.sh",
                "install",
            ])
            .kill_on_drop(true)
            .output()
            .await
            .with_context(|| format!("run wechat install in `{name}`"))?;
        if !output.status.success() {
            bail!(
                "wechat install failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        if !self.wechat_installed().await? {
            bail!("wechat install completed but the client is still not present");
        }
        Ok(())
    }

    /// Reports whether the instance container exists and is running.
    pub(crate) async fn is_running(&self) -> Result<bool> {
        let name = self.container_name();
        let output = self
            .docker(&["inspect", "-f", "{{.State.Running}}", &name])
            .await?;
        if !output.status.success() {
            // No such container is the common case — treat as "not running".
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "true")
    }

    /// Runs a bash script inside the container as the `abc` user with `DISPLAY`
    /// resolved, returning trimmed stdout. Fails if docker or the script fails.
    pub(crate) async fn exec_bash(&self, script: &str) -> Result<String> {
        let name = self.container_name();
        let full = format!("{DISPLAY_PRELUDE}{script}");
        let output = self
            .docker(&["exec", "--user", "abc", &name, "bash", "-lc", &full])
            .await?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker exec in `{name}` failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Runs a bash script inside the container as `abc` with `DISPLAY` resolved,
    /// feeding `stdin` to it, returning trimmed stdout. Used to push arbitrary
    /// UTF-8 text (message bodies, contact names) into `xclip` without any shell
    /// escaping or base64 round-trip.
    pub(crate) async fn exec_bash_stdin(&self, script: &str, stdin: &[u8]) -> Result<String> {
        let name = self.container_name();
        let full = format!("{DISPLAY_PRELUDE}{script}");
        let mut child = Command::new(&self.docker_bin)
            .args(["exec", "-i", "--user", "abc", &name, "bash", "-lc", &full])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn docker exec -i in `{name}`"))?;
        {
            let mut handle = child.stdin.take().context("docker exec stdin missing")?;
            handle.write_all(stdin).await.context("write exec stdin")?;
            handle.shutdown().await.ok();
        }
        let output = child
            .wait_with_output()
            .await
            .with_context(|| format!("await docker exec -i in `{name}`"))?;
        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            bail!("docker exec -i in `{name}` failed: {}", stderr.trim());
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    /// Ensures a screenshot tool (`import` from ImageMagick) is available in the
    /// container, installing it as root on first use. The WechatOnCloud image
    /// ships xdotool/xclip but no screen-capture tool.
    pub(crate) async fn ensure_screenshot_tool(&self) -> Result<()> {
        let name = self.container_name();
        let probe = self
            .docker(&[
                "exec",
                "--user",
                "abc",
                &name,
                "bash",
                "-lc",
                "command -v import >/dev/null 2>&1 && echo yes || echo no",
            ])
            .await?;
        if String::from_utf8_lossy(&probe.stdout).trim() == "yes" {
            return Ok(());
        }
        // Install as root (the runtime `abc` user cannot apt-get). Wait out any
        // concurrent apt (e.g. the dbread-deps install) first so the two root
        // apt-get runs don't collide on the dpkg lock.
        let output = self
            .docker(&[
                "exec",
                "--user",
                "root",
                &name,
                "bash",
                "-lc",
                "for i in $(seq 1 60); do fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break; sleep 2; done; \
                 apt-get update && apt-get install -y --no-install-recommends imagemagick",
            ])
            .await?;
        if !output.status.success() {
            bail!(
                "failed to install screenshot tool in `{name}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Desired capture geometry (`WIDTHxHEIGHT`), tunable via `WECHAT_SCREEN`
    /// (default 1280x800). Larger = sharper text for vision/OCR reads.
    fn capture_geometry(&self) -> (u32, u32) {
        std::env::var("WECHAT_SCREEN")
            .ok()
            .and_then(|value| {
                let (w, h) = value.split_once('x')?;
                Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
            })
            .unwrap_or((1280, 800))
    }

    /// Enlarges the virtual screen (via RandR) and the WeChat main window so
    /// screenshots are higher-resolution — the small default (~854x582) makes
    /// the vision model misread contact names. Best-effort; failures are ignored
    /// (e.g. the QR/login window is fixed-size and will not resize). Idempotent.
    pub(crate) async fn prepare_capture(&self) -> Result<()> {
        let (w, h) = self.capture_geometry();
        let win_h = h.saturating_sub(40);
        let script = format!(
            "xrandr --output VNC-0 --mode {w}x{h} 2>/dev/null || xrandr -s {w}x{h} 2>/dev/null || true; \
             wid=$(xdotool search --onlyvisible --name '微信|WeChat|Weixin' 2>/dev/null \
               | while read x; do eval \"$(xdotool getwindowgeometry --shell \"$x\" 2>/dev/null)\"; \
                 [ \"${{WIDTH:-0}}\" -gt 400 ] && echo \"$x\" && break; done); \
             if [ -n \"$wid\" ]; then \
               xdotool windowsize \"$wid\" {w} {win_h} 2>/dev/null || true; \
               xdotool windowmove \"$wid\" 0 0 2>/dev/null || true; \
             fi"
        );
        let _ = self.exec_bash(&script).await;
        Ok(())
    }

    /// Captures the whole virtual screen as PNG bytes (read-only — sends no
    /// input to WeChat). Returns raw bytes, not lossy text.
    pub(crate) async fn screenshot_png(&self) -> Result<Vec<u8>> {
        let name = self.container_name();
        let script = format!("{DISPLAY_PRELUDE}import -window root png:-");
        let output = self
            .docker(&["exec", "--user", "abc", &name, "bash", "-lc", &script])
            .await?;
        if !output.status.success() {
            bail!(
                "screenshot failed in `{name}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        // PNG magic number sanity check.
        if output.stdout.len() < 8 || &output.stdout[..4] != b"\x89PNG" {
            bail!("screenshot did not produce a PNG ({} bytes)", output.stdout.len());
        }
        Ok(output.stdout)
    }

    /// Runs `python3 - <args>` inside the container as `abc`, feeding `py` as the
    /// program on stdin. Returns (success, stdout, stderr). Used by the optional
    /// direct chat-DB reader. Output is captured as text (the script emits JSON).
    pub(crate) async fn exec_python(
        &self,
        py: &str,
        args: &[&str],
        env: &[(&str, &str)],
    ) -> Result<(bool, String, String)> {
        let name = self.container_name();
        // Run as ROOT: reading another process's /proc/<pid>/mem for key
        // extraction needs the container's CAP_SYS_PTRACE, which the non-root
        // `abc` user does not hold effectively. Root also reads the abc-owned DBs.
        // Secrets (e.g. SQLCipher keys) are passed via `-e NAME=VALUE`, not argv —
        // argv is world-readable via /proc/<pid>/cmdline (the untrusted WeChat
        // process runs as `abc`), whereas a root process's /proc/<pid>/environ is
        // not readable by `abc`.
        let mut argv: Vec<&str> = vec!["exec", "-i", "--user", "root"];
        let env_pairs: Vec<String> = env.iter().map(|(k, v)| format!("{k}={v}")).collect();
        for pair in &env_pairs {
            argv.push("-e");
            argv.push(pair);
        }
        argv.push(&name);
        argv.push("python3");
        argv.push("-");
        argv.extend_from_slice(args);
        let mut child = Command::new(&self.docker_bin)
            .args(&argv)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("spawn python in `{name}`"))?;
        {
            let mut handle = child.stdin.take().context("python stdin missing")?;
            handle.write_all(py.as_bytes()).await.context("write python source")?;
            handle.shutdown().await.ok();
        }
        let output = child.wait_with_output().await.context("await python in container")?;
        Ok((
            output.status.success(),
            String::from_utf8_lossy(&output.stdout).to_string(),
            String::from_utf8_lossy(&output.stderr).to_string(),
        ))
    }

    /// Ensures the Python crypto deps the DB reader needs are installed in the
    /// container (best-effort, one-time). Debian packages pycryptodome under the
    /// `Cryptodome` namespace, so we accept either `Crypto` or `Cryptodome`.
    pub(crate) async fn ensure_dbread_tools(&self) -> Result<()> {
        let name = self.container_name();
        // AES is required; check both namespaces.
        let probe = self
            .docker(&[
                "exec",
                "--user",
                "abc",
                &name,
                "python3",
                "-c",
                "import importlib.util,sys;sys.exit(0 if importlib.util.find_spec('Cryptodome') or importlib.util.find_spec('Crypto') else 1)",
            ])
            .await?;
        if probe.status.success() {
            return Ok(());
        }
        // Wait out any concurrent apt (e.g. the screenshot-tool install), then
        // install via apt (pip is not present and bookworm is PEP-668 managed).
        let script = "for i in $(seq 1 60); do fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break; sleep 2; done; \
             apt-get update >/dev/null 2>&1; \
             apt-get install -y --no-install-recommends python3-pycryptodome python3-zstandard >/dev/null 2>&1; \
             python3 -c 'import importlib.util,sys;sys.exit(0 if importlib.util.find_spec(\"Cryptodome\") or importlib.util.find_spec(\"Crypto\") else 1)'";
        let output = self
            .docker(&["exec", "--user", "root", &name, "bash", "-lc", script])
            .await?;
        if !output.status.success() {
            bail!(
                "could not install Python crypto deps (pycryptodome) for DB read in `{name}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
    }

    /// Reports whether the native WeChat process is running inside the container.
    #[allow(dead_code)] // used by the DB-read key-extraction + re-login watcher
    pub(crate) async fn wechat_process_running(&self) -> Result<bool> {
        // `pgrep` returns non-zero (exit 1) when nothing matches, which exec_bash
        // would surface as an error, so swallow that into a boolean explicitly.
        let name = self.container_name();
        let output = self
            .docker(&[
                "exec",
                "--user",
                "abc",
                &name,
                "bash",
                "-lc",
                "pgrep -f /config/wechat/opt/wechat/wechat >/dev/null 2>&1 && echo yes || echo no",
            ])
            .await?;
        if !output.status.success() {
            return Ok(false);
        }
        Ok(String::from_utf8_lossy(&output.stdout).trim() == "yes")
    }

    /// Checks whether WeChat is logged in by the LIVE visible-window width: the
    /// QR/login window is small (~280x380) while the logged-in main window is
    /// wide (~854x655+). The on-disk `/config/xwechat_files/wxid_*` marker is NOT
    /// used — it persists in the data volume after logout/recreate, so a freshly
    /// recreated container would falsely read as authenticated.
    pub(crate) async fn is_logged_in(&self) -> Result<bool> {
        if !self.is_running().await? {
            return Ok(false);
        }
        // Live signal: the QR/login window is small (~280x380); the logged-in
        // main window is large (~854x655). The on-disk `wxid_*` marker is NOT a
        // reliable signal — it persists in the data volume after logout/recreate,
        // so a freshly recreated container shows the QR yet still has the marker.
        // A wide, visible WeChat window means we are actually logged in now.
        let width = self.max_wechat_window_width().await.unwrap_or(0);
        Ok(width >= self.main_window_min_width())
    }

    /// Minimum visible-window width (px) taken to mean "logged in". The QR login
    /// window is ~280px; the main window is ~854px. Tunable via
    /// `WECHAT_MAIN_WINDOW_MIN_WIDTH` for other WeChat versions / DPI.
    fn main_window_min_width(&self) -> u32 {
        std::env::var("WECHAT_MAIN_WINDOW_MIN_WIDTH")
            .ok()
            .and_then(|value| value.trim().parse().ok())
            .unwrap_or(700)
    }

    /// Returns the current mouse position (x,y) inside the container's display.
    pub(crate) async fn mouse_pos(&self) -> Result<(i32, i32)> {
        let out = self
            .exec_bash("xdotool getmouselocation --shell")
            .await?;
        let mut x = 0;
        let mut y = 0;
        for line in out.lines() {
            if let Some(v) = line.strip_prefix("X=") {
                x = v.trim().parse().unwrap_or(0);
            } else if let Some(v) = line.strip_prefix("Y=") {
                y = v.trim().parse().unwrap_or(0);
            }
        }
        Ok((x, y))
    }

    /// Returns the width (px) of the widest visible WeChat window, or 0 if none.
    pub(crate) async fn max_wechat_window_width(&self) -> Result<u32> {
        let script = "maxw=0; \
            for w in $(xdotool search --onlyvisible --name '微信|WeChat|Weixin' 2>/dev/null); do \
              eval \"$(xdotool getwindowgeometry --shell \"$w\" 2>/dev/null)\"; \
              [ \"${WIDTH:-0}\" -gt \"$maxw\" ] && maxw=${WIDTH:-0}; \
            done; echo \"$maxw\"";
        let out = self.exec_bash(script).await?;
        Ok(out.trim().parse::<u32>().unwrap_or(0))
    }
}

/// Builds the `docker run` argv (everything after `docker`) for a fresh,
/// config-conforming WeChat container. Mirrors WechatOnCloud's `runInstance`
/// (data volume at `/config`, 1g shm, `seccomp=unconfined`, restart policy,
/// PUID/PGID/TZ/CUSTOM_USER/PASSWORD) but, having no panel, publishes the
/// KasmVNC web port to localhost so puffer's own browser pane can reach it.
fn run_args(cfg: &InstanceConfig, runtime: Runtime) -> Vec<String> {
    let container = cfg.container_name();
    let volume = cfg.volume_name();
    let mut args: Vec<String> = vec!["run".into(), "-d".into()];
    args.extend(["--name".into(), container.clone()]);
    if runtime == Runtime::Docker {
        // `container` has no --hostname flag.
        args.extend(["--hostname".into(), container]);
    }
    for (key, value) in [
        ("PUID", &cfg.puid),
        ("PGID", &cfg.pgid),
        ("TZ", &cfg.tz),
        ("CUSTOM_USER", &cfg.kasm_user),
        ("PASSWORD", &cfg.kasm_password),
    ] {
        args.push("-e".into());
        args.push(format!("{key}={value}"));
    }
    // Named data volume at /config (login + chat data persist across recreate).
    match runtime {
        Runtime::Docker => args.extend(["-v".into(), format!("{volume}:/config")]),
        // `container` uses --mount instead of -v.
        Runtime::Container => args.extend([
            "--mount".into(),
            format!("type=volume,source={volume},target=/config"),
        ]),
    }
    args.extend(["--shm-size".into(), "1g".into()]);
    if runtime == Runtime::Docker {
        // `container` does not accept --security-opt; its VM isolation makes the
        // seccomp relaxation unnecessary.
        args.extend(["--security-opt".into(), "seccomp=unconfined".into()]);
    }
    // Grant SYS_PTRACE (read other procs' /proc/<pid>/mem) only when the direct
    // chat-DB reader is enabled (on by default; `WECHAT_ENABLE_DB_READ=0` drops
    // both the reader and this cap on the container running the WeChat binary).
    if super::dbread::enabled() {
        args.extend(["--cap-add".into(), "SYS_PTRACE".into()]);
    }
    if runtime == Runtime::Docker {
        // `container` has no restart policy; the app/connector restarts it.
        args.extend(["--restart".into(), "unless-stopped".into()]);
    }
    // Publish only on loopback: KasmVNC basic-auth is the only gate, so never
    // expose the WeChat desktop on a routable interface.
    args.push("-p".into());
    args.push(format!("127.0.0.1:{}:3000", cfg.host_port));
    args.push(cfg.image.clone());
    args
}

/// Returns `key`'s env value if set and non-empty, otherwise `default`.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn container_name_is_prefixed() {
        let instance = WechatInstance {
            name: "default".to_string(),
            docker_bin: "docker".to_string(),
            runtime: Runtime::Docker,
        };
        assert_eq!(instance.container_name(), "puffer-wechat-default");
        assert_eq!(instance.name(), "default");
    }

    #[test]
    fn env_or_falls_back_on_empty() {
        assert_eq!(env_or("WECHAT_DOES_NOT_EXIST_XYZ", "fallback"), "fallback");
    }

    #[test]
    fn run_args_mirror_wechatoncloud_and_publish_loopback() {
        let cfg = InstanceConfig {
            instance: "default".to_string(),
            image: "ghcr.io/gloridust/wechat-on-cloud:latest".to_string(),
            host_port: 37042,
            kasm_user: "woc".to_string(),
            kasm_password: "pw".to_string(),
            puid: "1000".to_string(),
            pgid: "1000".to_string(),
            tz: "Asia/Shanghai".to_string(),
        };
        let args = run_args(&cfg, Runtime::Docker);
        let joined = args.join(" ");
        assert!(args.starts_with(&["run".to_string(), "-d".to_string()]));
        assert!(joined.contains("--name puffer-wechat-default"));
        assert!(joined.contains("-v puffer-wechat-default:/config"));
        assert!(joined.contains("--shm-size 1g"));
        assert!(joined.contains("--security-opt seccomp=unconfined"));
        assert!(joined.contains("-e CUSTOM_USER=woc"));
        assert!(joined.contains("-e PASSWORD=pw"));
        // Port is published ONLY on loopback.
        assert!(joined.contains("-p 127.0.0.1:37042:3000"));
        assert!(!joined.contains("0.0.0.0"));
        // Image is the final positional argument.
        assert_eq!(args.last().unwrap(), &cfg.image);
    }

    #[test]
    fn run_args_container_maps_unsupported_flags() {
        let cfg = InstanceConfig {
            instance: "default".to_string(),
            image: "puffer-wechat-atspi:4.1.1.4".to_string(),
            host_port: 37042,
            kasm_user: "woc".to_string(),
            kasm_password: "pw".to_string(),
            puid: "1000".to_string(),
            pgid: "1000".to_string(),
            tz: "Asia/Shanghai".to_string(),
        };
        let joined = run_args(&cfg, Runtime::Container).join(" ");
        // Apple `container`: volume via --mount, none of the docker-only flags.
        assert!(joined.contains("--mount type=volume,source=puffer-wechat-default,target=/config"));
        assert!(!joined.contains(" -v "));
        assert!(!joined.contains("--restart"));
        assert!(!joined.contains("--security-opt"));
        assert!(!joined.contains("--hostname"));
        // Still publishes loopback-only and passes the env + image through.
        assert!(joined.contains("-p 127.0.0.1:37042:3000"));
        assert!(joined.contains("-e CUSTOM_USER=woc"));
    }
}
