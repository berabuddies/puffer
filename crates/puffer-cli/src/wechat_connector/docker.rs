//! Container lifecycle and `exec` helpers for a managed WeChat desktop instance.
//!
//! A WeChat "instance" is one container named `puffer-wechat-<name>`, based on
//! the WechatOnCloud image (`ghcr.io/gloridust/wechat-on-cloud`), which bundles
//! Xvfb + openbox + KasmVNC + the native WeChat client plus `xdotool`/`xclip`.
//! Puffer talks to it through the `docker` or Apple `container` CLI — selected
//! at runtime (see [`select_runtime`]); their `run`/`exec` flags match — so
//! there is no new dependency (the browser worker likewise shells out to Chrome).
//!
//! All input/read commands run as the in-container `abc` user (the user WeChat
//! runs as) and export `DISPLAY` by probing `/tmp/.X11-unix` the same way the
//! WechatOnCloud panel does.

use anyhow::{bail, Context, Result};
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::config::InstanceConfig;

/// Default WeChat runtime image: the AT-SPI-capable, a11y-baked build produced by
/// `wechat-vz/image/build-image.sh` (current latest WeChat 4.1.1.7, verified to
/// expose the AT-SPI tree). The no-vision operate path NEEDS this image — the bare
/// WechatOnCloud base (`ghcr.io/gloridust/wechat-on-cloud:latest`) ships no
/// accessibility stack. Build/load it locally first; override per instance with
/// `WECHAT_IMAGE`.
pub(crate) const DEFAULT_IMAGE: &str = "puffer-wechat-atspi:4.1.1.7";

/// The pullable WechatOnCloud base image. It ships WeChat + the desktop but NO
/// accessibility stack. When the baked a11y image can't be built (e.g. Apple
/// `container`'s build VM can't reach the apt mirrors), the connector pulls this
/// and layers the a11y stack on at runtime (see [`WechatInstance::ensure_runtime_a11y`])
/// — a fully Docker-free path. Override with `WECHAT_BASE_IMAGE`.
const BASE_IMAGE: &str = "ghcr.io/gloridust/wechat-on-cloud:latest";

/// The a11y-enabled openbox autostart, embedded so the runtime-a11y fallback can
/// push it into a base container. This is the SAME script the baked image bakes
/// in (one source of truth): it sets the accessibility env + brings up the
/// session D-Bus / AT-SPI bus BEFORE (re)launching WeChat.
const RUNTIME_A11Y_AUTOSTART: &str = include_str!("../../wechat-vz/image/autostart");
/// The runtime-a11y apply script (apt the a11y stack + install the autostart),
/// embedded and pushed into the base container.
const RUNTIME_A11Y_APPLY_SH: &str = include_str!("../../wechat-vz/apply-runtime-a11y.sh");
/// In-container path the connector pushes [`RUNTIME_A11Y_AUTOSTART`] to (matches
/// the apply script's default `A11Y_AUTOSTART_SRC`).
const RUNTIME_A11Y_AUTOSTART_PATH: &str = "/tmp/puffer-a11y-autostart";
/// In-container path the connector pushes [`RUNTIME_A11Y_APPLY_SH`] to.
const RUNTIME_A11Y_APPLY_PATH: &str = "/tmp/puffer-apply-runtime-a11y.sh";

/// Bash run as ROOT before any runtime `apt-get`: if the Debian mirror doesn't
/// resolve, prepend a public resolver as primary and force DNS-over-TCP. Some
/// container DNS setups can't resolve external hosts (or block outbound UDP:53)
/// even when raw egress works, which otherwise hangs/fails apt. No-op when DNS
/// already works, and the change reverts on the next container restart. Mirrors
/// `wechat-vz/apply-runtime-a11y.sh`'s `ensure_dns`.
const APT_DNS_PRELUDE: &str = "if ! getent hosts deb.debian.org >/dev/null 2>&1; then \
     orig=\"$(grep -vE '^nameserver 8\\.8\\.8\\.8$|^options use-vc$' /etc/resolv.conf 2>/dev/null)\"; \
     printf 'nameserver 8.8.8.8\\n%s\\noptions use-vc\\n' \"$orig\" > /etc/resolv.conf 2>/dev/null || true; \
   fi; ";

/// apt-get options shared by the runtime install helpers: force IPv4, retry the
/// flaky mirror, bound the per-request timeout, and WAIT for the dpkg lock rather
/// than erroring when the base image's own boot apt holds it.
const APT_OPTS: &str =
    "-o Acquire::ForceIPv4=true -o Acquire::Retries=8 -o Acquire::http::Timeout=30 -o DPkg::Lock::Timeout=120";

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

/// How [`WechatInstance::ensure_image`] resolved the image to actually run: the
/// image reference plus whether the accessibility stack must be layered on at
/// runtime (true only for the base-image fallback; the baked image already has
/// it). [`WechatInstance::ensure_container`] runs the container from `image` and,
/// when `runtime_a11y` is set, applies the a11y setup after creating it.
struct ImagePlan {
    image: String,
    runtime_a11y: bool,
}

/// Resolves the runtime + its CLI binary from
/// `WECHAT_RUNTIME=docker|container|auto` (default `auto`). `auto` prefers Apple
/// `container` on macOS 26+ when it is installed — it ships only there and needs
/// no Docker Desktop — and falls back to Docker otherwise. `DOCKER_BIN` /
/// `WECHAT_CONTAINER_BIN` override the resolved binaries.
fn select_runtime() -> (Runtime, String) {
    match env_or("WECHAT_RUNTIME", "auto").to_ascii_lowercase().as_str() {
        "container" => (Runtime::Container, resolve_container_bin()),
        "docker" => (Runtime::Docker, env_or("DOCKER_BIN", "docker")),
        // `auto`: Apple `container` on macOS 26+ if installed, else Docker.
        _ => match auto_container_bin() {
            Some(bin) => (Runtime::Container, bin),
            None => (Runtime::Docker, env_or("DOCKER_BIN", "docker")),
        },
    }
}

/// The `container` binary for an explicit `WECHAT_RUNTIME=container`: the
/// `WECHAT_CONTAINER_BIN` override, else an absolute path resolved from `PATH`
/// or the standard Homebrew locations (so it works under the GUI app's minimal
/// `PATH`), else the bare name.
fn resolve_container_bin() -> String {
    container_bin_override()
        .or_else(locate_container_bin)
        .unwrap_or_else(|| "container".to_string())
}

/// For `WECHAT_RUNTIME=auto`: the `container` binary iff this is macOS 26+ and
/// the binary is installed, else `None` (→ Docker).
fn auto_container_bin() -> Option<String> {
    if !cfg!(target_os = "macos") || macos_major_version() < 26 {
        return None;
    }
    container_bin_override()
        .or_else(locate_container_bin)
        .or_else(brew_install_container)
}

/// On macOS 26+ where `auto` should use Apple `container` but it isn't installed,
/// install it once via Homebrew — the user only runs the app, no terminal. Best
/// effort, at most once per process: returns the resolved binary on success, or
/// `None` (→ Docker fallback). Opt out with `WECHAT_AUTO_INSTALL_CONTAINER=0`.
fn brew_install_container() -> Option<String> {
    use std::sync::OnceLock;
    static DONE: OnceLock<Option<String>> = OnceLock::new();
    DONE.get_or_init(|| {
        if matches!(std::env::var("WECHAT_AUTO_INSTALL_CONTAINER").as_deref(), Ok("0")) {
            return None;
        }
        let brew = which_on_path("brew").or_else(|| {
            ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
                .into_iter()
                .find(|p| is_executable_file(Path::new(p)))
                .map(str::to_string)
        })?;
        eprintln!(
            "wechat: Apple `container` not found — installing it via Homebrew (one-time, a few \
             minutes). Set WECHAT_AUTO_INSTALL_CONTAINER=0 to skip and use Docker."
        );
        let _ = std::process::Command::new(&brew).args(["install", "container"]).status();
        // The 1.0.0 bottle mislocated its plugins (apiserver crash-loop); the fix
        // shipped in 1.0.0_1, so update + upgrade past it.
        let _ = std::process::Command::new(&brew).arg("update").status();
        let _ = std::process::Command::new(&brew).args(["upgrade", "container"]).status();
        locate_container_bin()
    })
    .clone()
}

/// `WECHAT_CONTAINER_BIN`, if set and non-empty.
fn container_bin_override() -> Option<String> {
    std::env::var("WECHAT_CONTAINER_BIN")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

/// Absolute path to the `container` CLI from `PATH` or the standard Homebrew
/// locations, or `None` if it is not installed.
fn locate_container_bin() -> Option<String> {
    if let Some(path) = which_on_path("container") {
        return Some(path);
    }
    ["/opt/homebrew/bin/container", "/usr/local/bin/container"]
        .into_iter()
        .find(|path| is_executable_file(Path::new(path)))
        .map(str::to_string)
}

/// Whether `path` is an existing regular file with an execute bit set — so a
/// same-named non-executable file isn't mistaken for the CLI (it would only fail
/// with EACCES at exec time instead of falling through cleanly).
fn is_executable_file(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path)
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Runtimes whose teardown should be attempted when deleting a WeChat connection:
/// each of Docker and Apple `container` *when its CLI is installed*. The instance's
/// runtime can drift between create and delete (a `WECHAT_RUNTIME` change,
/// installing/removing a runtime, an OS upgrade), and the container + its named
/// data volume live on whichever runtime created them — so cleaning every PRESENT
/// runtime (the others harmlessly no-op) prevents a silent leak of the container
/// and its login/chat data. A runtime that isn't installed can't hold an instance,
/// so it is skipped entirely (no point spawning a missing binary — this keeps the
/// Docker-free Apple `container` path from ever invoking `docker`). Each entry is
/// `(binary, is_container)`.
pub(crate) fn teardown_runtimes() -> Vec<(String, bool)> {
    let mut runtimes = Vec::new();
    if let Some(bin) = locate_docker_bin() {
        runtimes.push((bin, false));
    }
    if let Some(bin) = container_bin_override().or_else(locate_container_bin) {
        runtimes.push((bin, true));
    }
    runtimes
}

/// Absolute path / name of the `docker` CLI from `DOCKER_BIN`, `PATH`, or the
/// standard install locations, or `None` if Docker is not installed (so a
/// Docker-free machine never tries to spawn it).
fn locate_docker_bin() -> Option<String> {
    if let Some(bin) = std::env::var("DOCKER_BIN").ok().map(|v| v.trim().to_string()).filter(|v| !v.is_empty()) {
        return Some(bin);
    }
    if let Some(path) = which_on_path("docker") {
        return Some(path);
    }
    ["/usr/local/bin/docker", "/opt/homebrew/bin/docker"]
        .into_iter()
        .find(|path| is_executable_file(Path::new(path)))
        .map(str::to_string)
}

/// Absolute path of `bin` if it is found on `PATH`.
fn which_on_path(bin: &str) -> Option<String> {
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|dir| dir.join(bin))
        .find(|candidate| is_executable_file(candidate))
        .map(|candidate| candidate.to_string_lossy().into_owned())
}

/// The major macOS version (e.g. `26`), cached for the process. `0` off macOS or
/// if the version can't be read.
fn macos_major_version() -> u32 {
    use std::sync::OnceLock;
    static VERSION: OnceLock<u32> = OnceLock::new();
    *VERSION.get_or_init(|| {
        std::process::Command::new("sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|out| {
                let text = String::from_utf8(out.stdout).ok()?;
                text.trim().split('.').next()?.parse().ok()
            })
            .unwrap_or(0)
    })
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

    /// Ensures a runnable WeChat image is present locally and returns the
    /// [`ImagePlan`] (the image to run + whether to layer a11y on at runtime).
    ///
    /// Priority is the baked a11y image (`cfg.image`): already present → `pull` →
    /// `load` from `WECHAT_CONTAINER_IMAGE_TAR` → `build` it. If the baked image
    /// can't be obtained, fall back (Docker-free) to the pullable BASE image plus
    /// runtime accessibility setup — and persist that choice so later runs skip
    /// straight to it. A clear, actionable error is returned only when even the
    /// base image can't be pulled.
    async fn ensure_image(&self, cfg: &InstanceConfig) -> Result<ImagePlan> {
        // A config that already fell back carries the base image + flag; honor it
        // without re-attempting the (failed) baked build.
        let on_base = cfg.image == base_image();
        let keep = ImagePlan { image: cfg.image.clone(), runtime_a11y: cfg.runtime_a11y };

        if self.runtime == Runtime::Container {
            // `container` has its own image store (separate from Docker's).
            // Resolution: already present → `pull` (registry ref) → `load` from a
            // configured OCI archive (`WECHAT_CONTAINER_IMAGE_TAR`).
            if self.image_present(&cfg.image).await? {
                return Ok(keep);
            }
            let _ = self.docker(&["image", "pull", &cfg.image]).await;
            if self.image_present(&cfg.image).await? {
                return Ok(keep);
            }
            if let Ok(tar) = std::env::var("WECHAT_CONTAINER_IMAGE_TAR") {
                let tar = tar.trim();
                if !tar.is_empty() {
                    let _ = self.docker(&["image", "load", "-i", tar]).await;
                    if self.image_present(&cfg.image).await? {
                        return Ok(keep);
                    }
                }
            }
            // Priority: build the baked a11y image (skipped if the config already
            // targets the base image — that build doesn't apply to it).
            if !on_base && self.try_build_atspi_image(cfg).await? {
                return Ok(ImagePlan { image: cfg.image.clone(), runtime_a11y: false });
            }
            // Fallback: base image + runtime a11y (fully Docker-free).
            if let Some(plan) = self.fall_back_to_base(cfg).await? {
                return Ok(plan);
            }
            bail!(
                "wechat image `{}` is not in the `container` image store and could not be \
                 obtained: `container image pull` failed, no WECHAT_CONTAINER_IMAGE_TAR OCI \
                 archive was set, building the baked a11y image failed, and the base image \
                 `{}` could not be pulled either. Check the machine's network / registry \
                 access (the `container` image store is separate from Docker's).",
                cfg.image,
                base_image()
            );
        }

        // Docker runtime.
        if self.image_present(&cfg.image).await? {
            return Ok(keep);
        }
        if let Ok(context) = std::env::var("WECHAT_BUILD_CONTEXT") {
            let context = context.trim();
            if !context.is_empty() {
                let output = self.docker(&["build", "-t", &cfg.image, context]).await?;
                if output.status.success() {
                    return Ok(keep);
                }
                bail!(
                    "failed to build wechat image `{}` from {context}: {}",
                    cfg.image,
                    String::from_utf8_lossy(&output.stderr).trim()
                );
            }
        }
        let pull = self.docker(&["pull", &cfg.image]).await?;
        if pull.status.success() {
            return Ok(keep);
        }
        if !on_base && self.try_build_atspi_image(cfg).await? {
            return Ok(ImagePlan { image: cfg.image.clone(), runtime_a11y: false });
        }
        if let Some(plan) = self.fall_back_to_base(cfg).await? {
            return Ok(plan);
        }
        bail!(
            "wechat image `{}` is not available: not built locally, no WECHAT_BUILD_CONTEXT \
             set to build it, `docker pull` failed ({}), and the base image `{}` could not be \
             pulled for the runtime-accessibility fallback either.",
            cfg.image,
            String::from_utf8_lossy(&pull.stderr).trim(),
            base_image()
        )
    }

    /// The Docker-free fallback when the baked a11y image can't be obtained: pull
    /// the base image and, on success, persist the choice (image := base,
    /// `runtime_a11y` := true) so later runs go straight to it, returning the plan
    /// to run base + layer a11y on at runtime. `None` (caller bails) if the base
    /// image can't be pulled or the fallback is disabled (`WECHAT_RUNTIME_A11Y=0`).
    async fn fall_back_to_base(&self, cfg: &InstanceConfig) -> Result<Option<ImagePlan>> {
        if matches!(std::env::var("WECHAT_RUNTIME_A11Y").as_deref(), Ok("0")) {
            return Ok(None);
        }
        let base = base_image();
        eprintln!(
            "wechat: the baked accessibility image is unavailable; falling back to the base \
             image `{base}` + a one-time runtime accessibility setup (Docker-free)…"
        );
        // Pull (the docker `pull`/container `image pull` verbs differ; container
        // accepts `image pull`, docker accepts both, so use `image pull` for both).
        let _ = self.docker(&["image", "pull", &base]).await;
        if !self.image_present(&base).await? {
            return Ok(None);
        }
        // Persist so the act/subscribe paths and later starts skip the failed
        // baked build and re-apply a11y on the base image. Best-effort.
        let mut updated = cfg.clone();
        updated.image = base.clone();
        updated.runtime_a11y = true;
        if let Err(error) = updated.save() {
            eprintln!("wechat: could not persist the runtime-a11y fallback config: {error:#}");
        }
        Ok(Some(ImagePlan { image: base, runtime_a11y: true }))
    }

    /// Auto-builds the AT-SPI WeChat image when it is missing. That image bakes the
    /// accessibility stack + WeChat and is in no registry, so a fresh machine can't
    /// `pull` it. Invokes the repo build pipeline (`wechat-vz/image/build-image.sh`:
    /// fetches the latest Universal WeChat .deb, layers the a11y stack, tags it as
    /// `cfg.image`) with the selected runtime's native builder — `docker build` for
    /// Docker, `container build` (its own builder VM) for Apple `container`, so
    /// neither runtime needs the other. Returns whether the image is present
    /// afterwards. Best-effort; the build context is the crate's source tree, so a
    /// packaged binary with no source tree skips this (the caller then falls back
    /// to the base image + runtime accessibility setup).
    async fn try_build_atspi_image(&self, cfg: &InstanceConfig) -> Result<bool> {
        let script = concat!(env!("CARGO_MANIFEST_DIR"), "/wechat-vz/image/build-image.sh");
        if !Path::new(script).exists() {
            return Ok(false);
        }
        eprintln!(
            "wechat: image `{}` not found — building it now (first run fetches WeChat + builds; \
             several minutes)…",
            cfg.image
        );
        let runtime = if self.runtime == Runtime::Container { "container" } else { "docker" };
        let mut cmd = Command::new("bash");
        cmd.arg(script)
            .env("WECHAT_RUNTIME", runtime)
            .env("WECHAT_ATSPI_IMAGE", &cfg.image)
            .env("DOCKER_BIN", env_or("DOCKER_BIN", "docker"))
            .kill_on_drop(true);
        if self.runtime == Runtime::Container {
            cmd.env("WECHAT_CONTAINER_BIN", &self.docker_bin);
        }
        let _ = cmd.status().await;
        self.image_present(&cfg.image).await
    }

    /// Ensures the container is running: starts it if stopped, creates it (and
    /// pulls the image) if missing. Idempotent — safe to call before every
    /// `act`/`subscribe`. The data volume persists login across recreates.
    pub(crate) async fn ensure_container(&self, cfg: &InstanceConfig) -> Result<()> {
        // The engine/runtime itself may be off on a fresh machine — start it first.
        self.ensure_runtime_ready().await?;
        if self.is_running().await? {
            // Running already; if this is a runtime-a11y (base-image) instance,
            // make sure the a11y stack is in place (idempotent fast no-op once up).
            if cfg.runtime_a11y {
                self.ensure_runtime_a11y().await?;
            }
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
                if cfg.runtime_a11y {
                    self.ensure_runtime_a11y().await?;
                }
                return Ok(());
            }
            // Stale/inconsistent record — remove it so the recreate below is clean.
            let _ = self.docker(&["rm", "-f", &name]).await;
        }
        let plan = self.ensure_image(cfg).await?;
        self.create_container(cfg, &plan.image).await?;
        // The base-image fallback ships no accessibility stack — layer it on now
        // (apt the a11y stack + install the a11y autostart, then restart so WeChat
        // (re)launches under it). The baked image already has it, so this is a
        // no-op for the normal path.
        if plan.runtime_a11y {
            self.ensure_runtime_a11y().await?;
        }
        Ok(())
    }

    /// Creates and starts a fresh container from `image` (the [`ImagePlan`]'s
    /// resolved image, which may be the baked a11y image or the base-image
    /// fallback), recovering from a name conflict (a leftover container by the
    /// same name) by removing it and retrying once.
    async fn create_container(&self, cfg: &InstanceConfig, image: &str) -> Result<()> {
        let args = run_args(cfg, self.runtime, image);
        let mut output = self.docker_args(&args).await?;
        // Apple `container`: the very first `run` on a fresh setup fails until a
        // VM kernel is configured. Install the recommended kernel once, then
        // retry. (We do this lazily here rather than on every startup because
        // `kernel set` re-downloads each call.)
        if !output.status.success()
            && self.runtime == Runtime::Container
            && stderr_needs_kernel(&output.stderr)
        {
            if let Ok(kr) = self.docker(&["system", "kernel", "set", "--recommended"]).await {
                if !kr.status.success() {
                    eprintln!(
                        "wechat: `container system kernel set --recommended` failed: {}",
                        String::from_utf8_lossy(&kr.stderr).trim()
                    );
                }
            }
            output = self.docker_args(&args).await?;
        }
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

    /// Layers the AT-SPI accessibility stack onto a base-image container at
    /// runtime (the Docker-free fallback for when the baked a11y image can't be
    /// built). Idempotent and cheap once a11y is live: it first probes the live
    /// a11y bus and returns immediately if it is up. Otherwise it pushes + runs
    /// the apply script (apt the a11y stack + install the a11y openbox autostart)
    /// and, if that changed anything, restarts the container so openbox brings up
    /// the bus and (re)launches WeChat under the accessibility env.
    async fn ensure_runtime_a11y(&self) -> Result<()> {
        if self.runtime_a11y_live().await {
            return Ok(());
        }
        eprintln!(
            "wechat: setting up accessibility inside the container (one-time; installs the \
             AT-SPI stack, ~1 min)…"
        );
        // Push the a11y autostart (single source of truth, shared with the baked
        // image) and the apply script as `abc`; the apply script then runs as root.
        self.exec_bash_stdin(
            &format!("cat > {RUNTIME_A11Y_AUTOSTART_PATH}"),
            RUNTIME_A11Y_AUTOSTART.as_bytes(),
        )
        .await
        .context("push the a11y autostart into the container")?;
        self.exec_bash_stdin(
            &format!("cat > {RUNTIME_A11Y_APPLY_PATH}"),
            RUNTIME_A11Y_APPLY_SH.as_bytes(),
        )
        .await
        .context("push the runtime-a11y apply script into the container")?;

        let name = self.container_name();
        let output = self
            .docker(&["exec", "--user", "root", &name, "bash", RUNTIME_A11Y_APPLY_PATH])
            .await?;
        if !output.status.success() {
            bail!(
                "runtime accessibility setup failed in `{name}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.contains("A11Y_READY=0") {
            // The a11y stack didn't install (e.g. the runtime container couldn't
            // reach the apt mirrors). Don't fail bringup — login + DB reads still
            // work — but warn: the no-vision operate path (send) needs it, and
            // with vision off it will otherwise fail closed with a vaguer error.
            eprintln!(
                "wechat: WARNING — could not install the accessibility stack in the container; \
                 message-send (the no-vision operate path) will not work until it installs. \
                 Check the container's network/apt access."
            );
        }
        let changed = stdout.contains("A11Y_CHANGED=1");
        if changed {
            // Restart so the boot hook installs our autostart and openbox launches
            // WeChat under the a11y env. WeChat may not be installed yet (the setup
            // flow installs it next); the autostart brings up the bus first and
            // waits for the binary, so a11y is ready either way.
            let _ = self.docker(&["stop", "-t", "5", &name]).await;
            let started = self.docker(&["start", &name]).await?;
            if !started.status.success() {
                bail!(
                    "could not restart `{name}` after accessibility setup: {}",
                    String::from_utf8_lossy(&started.stderr).trim()
                );
            }
            // Best-effort: wait for the a11y bus to come up. Not fatal if it lags —
            // the setup flow's window/login waits tolerate a slow desktop, and the
            // next call re-checks. Bounded so a wedged boot doesn't hang setup.
            for _ in 0..40 {
                if self.runtime_a11y_live().await {
                    break;
                }
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
        Ok(())
    }

    /// Reports whether the in-container AT-SPI bus is live: the bus launcher is
    /// running and the autostart recorded its session-bus address. This reflects
    /// the ACTUAL running container (correct even after a recreate reset the
    /// rootfs), so it is a reliable idempotency guard for [`Self::ensure_runtime_a11y`].
    async fn runtime_a11y_live(&self) -> bool {
        let name = self.container_name();
        self.docker(&[
            "exec",
            "--user",
            "abc",
            &name,
            "bash",
            "-lc",
            "pgrep -f at-spi-bus-launcher >/dev/null 2>&1 && [ -s /run/wechat/dbus-addr ]",
        ])
        .await
        .map(|out| out.status.success())
        .unwrap_or(false)
    }

    /// Brings the selected runtime up and ready before any container op, so the
    /// user never has to open a terminal. Docker launches the engine; Apple
    /// `container` starts (and, on a Homebrew install, repairs) its apiserver.
    async fn ensure_runtime_ready(&self) -> Result<()> {
        match self.runtime {
            Runtime::Docker => self.ensure_docker_daemon().await,
            Runtime::Container => self.ensure_container_runtime().await,
        }
    }

    /// Ensures Apple `container`'s background apiserver is up and answering,
    /// starting it if needed. The VM kernel is installed lazily on the first
    /// `create` (see [`Self::create_container`]), since `kernel set` re-downloads
    /// each call. Idempotent and cheap once the apiserver is healthy.
    async fn ensure_container_runtime(&self) -> Result<()> {
        if self.container_runtime_ready().await {
            return Ok(());
        }
        // Bring the apiserver up (idempotent — a no-op if another start raced us).
        let _ = self.docker(&["system", "start"]).await;
        for _ in 0..30 {
            if self.container_runtime_ready().await {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
        bail!(
            "Apple `container` apiserver did not respond after `container system start`. \
             If it reports a missing `network` plugin, the Homebrew `container` 1.0.0 \
             package mislocated its plugins; update past it with \
             `brew update && brew upgrade container` (the fix shipped in 1.0.0_1)."
        )
    }

    /// Reports whether `container system status` answers "running". That command
    /// hangs while the apiserver is wedged, so it is bounded by a timeout (a
    /// cancelled call is killed via `kill_on_drop`).
    async fn container_runtime_ready(&self) -> bool {
        match tokio::time::timeout(Duration::from_secs(8), self.docker(&["system", "status"])).await
        {
            Ok(Ok(out)) => {
                out.status.success() && String::from_utf8_lossy(&out.stdout).contains("running")
            }
            _ => false,
        }
    }

    /// The `host:port` authority where this instance's KasmVNC desktop is
    /// reachable from the host. Docker publishes the web port to loopback; Apple
    /// `container` does NOT forward published ports — the desktop is reachable
    /// only at the container's vmnet IP — so resolve that IP (falling back to
    /// loopback if it can't be read).
    pub(crate) async fn desktop_authority(&self, cfg: &InstanceConfig) -> String {
        if self.runtime == Runtime::Container {
            // The vmnet IPv4 lease appears a moment after the VM boots, so it can
            // be absent right after `run -d` returns — poll briefly.
            for attempt in 0..10 {
                if let Some(ip) = self.container_ip().await {
                    return format!("{ip}:3000");
                }
                if attempt < 9 {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
            }
            // `container` publishes no host port, so loopback is a dead address
            // here — surface that rather than hand back a silently-broken URL.
            eprintln!(
                "wechat: could not read the container's vmnet IP from `container inspect`; \
                 the WeChat desktop URL may be unreachable"
            );
        }
        format!("127.0.0.1:{}", cfg.host_port)
    }

    /// The container's vmnet IPv4 address (Apple `container` only), without the
    /// CIDR suffix, or `None` if it can't be determined.
    async fn container_ip(&self) -> Option<String> {
        let name = self.container_name();
        let out = self.docker(&["inspect", &name]).await.ok()?;
        if !out.status.success() {
            return None;
        }
        let value: serde_json::Value = serde_json::from_slice(&out.stdout).ok()?;
        let entry = value.as_array().and_then(|items| items.first()).unwrap_or(&value);
        let addr = entry
            .pointer("/status/networks/0/ipv4Address")
            .and_then(serde_json::Value::as_str)?;
        Some(addr.split('/').next().unwrap_or(addr).to_string())
    }

    /// Reports whether `image` is present in the selected runtime's image store.
    /// Both `docker` and `container` answer `image inspect <ref>` with success iff
    /// the image is local.
    async fn image_present(&self, image: &str) -> Result<bool> {
        Ok(self
            .docker(&["image", "inspect", image])
            .await?
            .status
            .success())
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
        match self.runtime {
            Runtime::Docker => {
                let output = self
                    .docker(&["inspect", "-f", "{{.State.Running}}", &name])
                    .await?;
                // No such container is the common case — treat as "not running".
                Ok(output.status.success()
                    && String::from_utf8_lossy(&output.stdout).trim() == "true")
            }
            Runtime::Container => {
                // Apple `container inspect` has no `-f` flag; its JSON exposes the
                // run state at /status/state ("running" | "stopped").
                let output = self.docker(&["inspect", &name]).await?;
                if !output.status.success() {
                    return Ok(false);
                }
                let Ok(value) = serde_json::from_slice::<serde_json::Value>(&output.stdout) else {
                    return Ok(false);
                };
                let entry = value.as_array().and_then(|items| items.first()).unwrap_or(&value);
                Ok(entry.pointer("/status/state").and_then(serde_json::Value::as_str)
                    == Some("running"))
            }
        }
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
        // The screenshot tool feeds ONLY the vision path (screen reads + the
        // optional act delivery/recipient vision checks). With vision off — the
        // default; the no-vision a11y path handles those — it is never used, so
        // skip the install entirely. This also avoids wasting the caller's time
        // budget on a (possibly slow, network-dependent) apt that isn't needed.
        if !super::read::vision_allowed() {
            return Ok(());
        }
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
        // Install as root (the runtime `abc` user cannot apt-get). Ensure DNS,
        // then wait out any concurrent apt holding the dpkg OR apt-lists lock (the
        // base image runs its own boot apt) before installing.
        let script = format!(
            "{APT_DNS_PRELUDE}\
             for i in $(seq 1 90); do fuser /var/lib/dpkg/lock-frontend /var/lib/apt/lists/lock >/dev/null 2>&1 || break; sleep 2; done; \
             apt-get {APT_OPTS} update && apt-get {APT_OPTS} install -y --no-install-recommends imagemagick"
        );
        let output = self
            .docker(&["exec", "--user", "root", &name, "bash", "-lc", &script])
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
        // Ensure DNS, wait out any concurrent apt holding the dpkg OR apt-lists
        // lock, then install via apt (pip is not present and bookworm is PEP-668
        // managed). The baked image pre-installs these, so this only runs on the
        // base-image runtime-a11y path.
        let script = format!(
            "{APT_DNS_PRELUDE}\
             for i in $(seq 1 90); do fuser /var/lib/dpkg/lock-frontend /var/lib/apt/lists/lock >/dev/null 2>&1 || break; sleep 2; done; \
             apt-get {APT_OPTS} update >/dev/null 2>&1; \
             apt-get {APT_OPTS} install -y --no-install-recommends python3-pycryptodome python3-zstandard >/dev/null 2>&1; \
             python3 -c 'import importlib.util,sys;sys.exit(0 if importlib.util.find_spec(\"Cryptodome\") or importlib.util.find_spec(\"Crypto\") else 1)'"
        );
        let output = self
            .docker(&["exec", "--user", "root", &name, "bash", "-lc", &script])
            .await?;
        if !output.status.success() {
            bail!(
                "could not install Python crypto deps (pycryptodome) for DB read in `{name}`: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(())
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
fn run_args(cfg: &InstanceConfig, runtime: Runtime, image: &str) -> Vec<String> {
    let container = cfg.container_name();
    let volume = cfg.volume_name();
    let mut args: Vec<String> = vec!["run".into(), "-d".into()];
    args.extend(["--name".into(), container.clone()]);
    if runtime == Runtime::Docker {
        // `container` has no --hostname flag.
        args.extend(["--hostname".into(), container]);
    }
    // PUID/PGID remap the in-container `abc` user to the given uid/gid. On Apple
    // `container` the linuxserver remap is applied non-deterministically across
    // boots — `abc`'s uid drifts between the image default and PUID — so the
    // persisted login data (mode 700) ends up owned by a uid a later boot can't
    // read, and WeChat exits 255 / the a11y bus fails to bind. The VM-backed
    // store needs no host-uid matching, so skip the remap there: `abc` keeps a
    // stable uid and ownership stays consistent across restarts. Docker (where
    // the remap is reliable and host-uid matching can matter) keeps it.
    let mut env: Vec<(&str, &str)> = Vec::new();
    if runtime == Runtime::Docker {
        env.push(("PUID", &cfg.puid));
        env.push(("PGID", &cfg.pgid));
    }
    env.push(("TZ", &cfg.tz));
    env.push(("CUSTOM_USER", &cfg.kasm_user));
    env.push(("PASSWORD", &cfg.kasm_password));
    for (key, value) in env {
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
    if runtime == Runtime::Docker {
        // Docker: publish only on loopback (KasmVNC basic-auth is the only gate,
        // so never expose the WeChat desktop on a routable interface).
        args.push("-p".into());
        args.push(format!("127.0.0.1:{}:3000", cfg.host_port));
    }
    // Apple `container` reaches the desktop at the container's vmnet IP (see
    // `desktop_authority`), so it needs no published host port — and `-p` would
    // also collide on the host port with a Docker instance of the same slug.
    args.push(image.to_string());
    args
}

/// Returns `key`'s env value if set and non-empty, otherwise `default`.
fn env_or(key: &str, default: &str) -> String {
    std::env::var(key)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| default.to_string())
}

/// The pullable base image for the runtime-a11y fallback (`WECHAT_BASE_IMAGE`
/// override, else [`BASE_IMAGE`]).
fn base_image() -> String {
    env_or("WECHAT_BASE_IMAGE", BASE_IMAGE)
}

/// Whether a `container run` failure is the fresh-setup "no VM kernel" error,
/// which is cleared by `container system kernel set --recommended`.
fn stderr_needs_kernel(stderr: &[u8]) -> bool {
    let s = String::from_utf8_lossy(stderr);
    s.contains("kernel not configured") || s.contains("kernel is not configured")
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
    fn stderr_needs_kernel_detects_fresh_setup() {
        assert!(stderr_needs_kernel(
            b"Error: default kernel not configured for architecture arm64, please use the \
              `container system kernel set` command to configure it"
        ));
        assert!(!stderr_needs_kernel(b"Error: container name already in use"));
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
            runtime_a11y: false,
        };
        let args = run_args(&cfg, Runtime::Docker, &cfg.image);
        let joined = args.join(" ");
        assert!(args.starts_with(&["run".to_string(), "-d".to_string()]));
        assert!(joined.contains("--name puffer-wechat-default"));
        assert!(joined.contains("-v puffer-wechat-default:/config"));
        assert!(joined.contains("--shm-size 1g"));
        assert!(joined.contains("--security-opt seccomp=unconfined"));
        // Docker keeps the PUID/PGID remap.
        assert!(joined.contains("-e PUID=1000"));
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
            image: "puffer-wechat-atspi:4.1.1.7".to_string(),
            host_port: 37042,
            kasm_user: "woc".to_string(),
            kasm_password: "pw".to_string(),
            puid: "1000".to_string(),
            pgid: "1000".to_string(),
            tz: "Asia/Shanghai".to_string(),
            runtime_a11y: false,
        };
        let joined = run_args(&cfg, Runtime::Container, &cfg.image).join(" ");
        // Apple `container`: volume via --mount, none of the docker-only flags.
        assert!(joined.contains("--mount type=volume,source=puffer-wechat-default,target=/config"));
        assert!(!joined.contains(" -v "));
        assert!(!joined.contains("--restart"));
        assert!(!joined.contains("--security-opt"));
        assert!(!joined.contains("--hostname"));
        // No published host port — the desktop is reached at the vmnet IP, and
        // `-p` would collide with a Docker instance of the same slug.
        assert!(!joined.contains("-p "));
        // No PUID/PGID remap on `container` (keeps `abc`'s uid stable across boots).
        assert!(!joined.contains("PUID"));
        assert!(!joined.contains("PGID"));
        assert!(joined.contains("-e CUSTOM_USER=woc"));
    }

    #[test]
    fn run_args_uses_the_plan_image_not_cfg_image() {
        // The runtime-a11y fallback runs the BASE image while cfg.image still
        // names the (unobtainable) baked image — so run_args must honor the
        // explicit image it is handed, not cfg.image.
        let cfg = InstanceConfig {
            instance: "default".to_string(),
            image: "puffer-wechat-atspi:4.1.1.7".to_string(),
            host_port: 37042,
            kasm_user: "woc".to_string(),
            kasm_password: "pw".to_string(),
            puid: "1000".to_string(),
            pgid: "1000".to_string(),
            tz: "Asia/Shanghai".to_string(),
            runtime_a11y: true,
        };
        let base = "ghcr.io/gloridust/wechat-on-cloud:latest";
        let args = run_args(&cfg, Runtime::Container, base);
        assert_eq!(args.last().unwrap(), base);
        assert!(!args.iter().any(|a| a == &cfg.image));
    }
}
