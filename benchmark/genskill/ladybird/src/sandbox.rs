//! Spawns and tears down replay sandboxes via the docker CLI.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use tokio::process::Command;

/// Image tag built by Dockerfile.ladybird-eval.
pub const DEFAULT_IMAGE: &str = "puffer-genskill-eval-ladybird";

/// Working directory inside the container where ladybird is checked out.
pub const CONTAINER_WORKDIR: &str = "/work/ladybird";

const CONTAINER_USER: &str = "ladybird";

/// One running replay sandbox. Drop releases the container.
pub struct Sandbox {
    container_id: String,
}

/// Result of running a command inside the sandbox.
pub struct ExecOutput {
    /// Process stdout decoded as UTF-8 lossily.
    pub stdout: String,
    /// Process stderr decoded as UTF-8 lossily.
    pub stderr: String,
    /// Process exit status code, or -1 if Docker did not report one.
    pub exit_code: i32,
}

impl Sandbox {
    /// Spawns a fresh container, checks out `base_commit`, and copies test
    /// files in. The puffer binary at `puffer_bin_host_path` is mounted
    /// read-only at /usr/local/bin/puffer.
    pub async fn start(
        image: &str,
        puffer_bin_host_path: &Path,
        base_commit: &str,
        test_files_host_dir: &Path,
    ) -> Result<Self> {
        let puffer_bin_abs = puffer_bin_host_path.canonicalize().with_context(|| {
            format!(
                "canonicalizing puffer binary path {}",
                puffer_bin_host_path.display()
            )
        })?;
        let test_files_abs = test_files_host_dir.canonicalize().with_context(|| {
            format!(
                "canonicalizing test files dir {}",
                test_files_host_dir.display()
            )
        })?;
        let host_root = std::env::current_dir().context("resolving host workspace root")?;
        let host_codex_dir = host_codex_dir();
        let vcpkg_cache_dir = host_vcpkg_binary_cache_dir(&host_root)?;
        let compiler_cache_dir = host_compiler_cache_dir(&host_root)?;

        let mut cmd = Command::new("docker");
        cmd.args(["run", "-d", "--rm"])
            .args([
                "-v",
                &format!("{}:/usr/local/bin/puffer:ro", puffer_bin_abs.display()),
            ])
            .args([
                "-v",
                &format!("{}:/work/test_files:ro", test_files_abs.display()),
            ])
            .args([
                "-v",
                &format!("{}:/work/vcpkg-binary-cache", vcpkg_cache_dir.display()),
            ])
            .args([
                "-v",
                &format!("{}:/work/ccache", compiler_cache_dir.display()),
            ])
            .args(["-v", &format!("{}:/host:ro", host_root.display())])
            .args(["-e", "HOME=/home/ladybird"])
            .args([
                "-e",
                "VCPKG_BINARY_SOURCES=clear;files,/work/vcpkg-binary-cache,readwrite",
            ])
            .args(["-e", "CCACHE_DIR=/work/ccache"])
            .args(["-e", "CCACHE_BASEDIR=/work/ladybird"])
            .args(["-e", "CCACHE_COMPILERCHECK=content"])
            .args(["-e", "CCACHE_MAXSIZE=20G"])
            .args(["-e", "CMAKE_C_COMPILER_LAUNCHER=ccache"])
            .args(["-e", "CMAKE_CXX_COMPILER_LAUNCHER=ccache"]);
        for name in [
            "OPENAI_API_KEY",
            "ANTHROPIC_API_KEY",
            "OPENAI_BASE_URL",
            "OPENAI_ORGANIZATION",
            "OPENAI_PROJECT",
            "PUFFER_PROVIDER",
            "PUFFER_MODEL",
            "PUFFER_EFFORT",
            "PUFFER_EVAL_PROVIDER",
            "PUFFER_EVAL_MODEL",
            "PUFFER_EVAL_EFFORT",
        ] {
            cmd.args(["-e", name]);
        }
        if let Some(codex_dir) = host_codex_dir {
            cmd.arg("-v")
                .arg(format!("{}:/home/ladybird/.codex:ro", codex_dir.display()));
        }
        let out = cmd
            .args(["--user", CONTAINER_USER])
            .args(["--workdir", CONTAINER_WORKDIR])
            .arg(image)
            .args(["sleep", "infinity"])
            .stdout(Stdio::piped())
            .output()
            .await
            .context("spawning docker run")?;
        if !out.status.success() {
            return Err(anyhow!(
                "docker run failed: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        let container_id = String::from_utf8(out.stdout)?.trim().to_string();
        if container_id.is_empty() {
            return Err(anyhow!("empty container id from docker run"));
        }
        let sandbox = Self { container_id };
        sandbox
            .exec(&["git", "reset", "--hard", base_commit])
            .await?;
        sandbox
            .exec(&["bash", "-c", "cp -r /work/test_files/. /work/ladybird/"])
            .await?;
        Ok(sandbox)
    }

    /// Runs a command inside the container, returning (stdout, stderr).
    pub async fn exec(&self, argv: &[&str]) -> Result<(String, String)> {
        let out = self.exec_status(argv).await?;
        if out.exit_code != 0 {
            return Err(anyhow!(
                "docker exec failed (status {}): {}",
                out.exit_code,
                out.stderr
            ));
        }
        Ok((out.stdout, out.stderr))
    }

    /// Runs a command inside the container without treating non-zero exit as
    /// an error. Docker invocation errors are still returned as errors.
    pub async fn exec_status(&self, argv: &[&str]) -> Result<ExecOutput> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec")
            .args(["--user", CONTAINER_USER])
            .arg(&self.container_id);
        for a in argv {
            cmd.arg(a);
        }
        let out = cmd.output().await.context("docker exec")?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        Ok(ExecOutput {
            stdout,
            stderr,
            exit_code: out.status.code().unwrap_or(-1),
        })
    }

    /// Container id (for diagnostics).
    #[allow(dead_code)]
    pub fn container_id(&self) -> &str {
        &self.container_id
    }
}

fn host_codex_dir() -> Option<PathBuf> {
    let dir = std::env::var_os("HOME").map(PathBuf::from)?.join(".codex");
    dir.exists().then_some(dir)
}

fn host_vcpkg_binary_cache_dir(host_root: &Path) -> Result<PathBuf> {
    let dir = cache_dir_from_env(
        "PUFFER_LADYBIRD_VCPKG_BINARY_CACHE",
        host_root,
        "vcpkg-binary",
    );
    prepare_host_cache_dir(dir)
}

fn host_compiler_cache_dir(host_root: &Path) -> Result<PathBuf> {
    let dir = cache_dir_from_env("PUFFER_LADYBIRD_CCACHE_DIR", host_root, "ccache");
    prepare_host_cache_dir(dir)
}

fn cache_dir_from_env(env_name: &str, host_root: &Path, default_name: &str) -> PathBuf {
    std::env::var_os(env_name)
        .map(PathBuf::from)
        .unwrap_or_else(|| {
            host_root
                .join("benchmark/genskill/ladybird/.cache")
                .join(default_name)
        })
}

fn prepare_host_cache_dir(dir: PathBuf) -> Result<PathBuf> {
    std::fs::create_dir_all(&dir).with_context(|| format!("creating {}", dir.display()))?;
    make_cache_dir_writable(&dir)?;
    dir.canonicalize()
        .with_context(|| format!("canonicalizing {}", dir.display()))
}

#[cfg(unix)]
fn make_cache_dir_writable(dir: &Path) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    let mut permissions = std::fs::metadata(dir)
        .with_context(|| format!("reading metadata for {}", dir.display()))?
        .permissions();
    permissions.set_mode(0o777);
    std::fs::set_permissions(dir, permissions)
        .with_context(|| format!("setting permissions on {}", dir.display()))
}

#[cfg(not(unix))]
fn make_cache_dir_writable(_dir: &Path) -> Result<()> {
    Ok(())
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::process::Command::new("docker")
            .args(["rm", "-f", &self.container_id])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
