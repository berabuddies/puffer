//! Spawns and tears down replay sandboxes via the docker CLI.

use anyhow::{anyhow, Context, Result};
use std::path::Path;
use std::process::Stdio;
use tokio::process::Command;

/// Image tag built by Dockerfile.ladybird-eval.
pub const DEFAULT_IMAGE: &str = "puffer-genskill-eval-ladybird";

/// Working directory inside the container where ladybird is checked out.
pub const CONTAINER_WORKDIR: &str = "/work/ladybird";

/// One running replay sandbox. Drop releases the container.
pub struct Sandbox {
    container_id: String,
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
            format!("canonicalizing puffer binary path {}", puffer_bin_host_path.display())
        })?;
        let test_files_abs = test_files_host_dir.canonicalize().with_context(|| {
            format!("canonicalizing test files dir {}", test_files_host_dir.display())
        })?;

        let out = Command::new("docker")
            .args(["run", "-d", "--rm"])
            .args(["-v", &format!("{}:/usr/local/bin/puffer:ro", puffer_bin_abs.display())])
            .args(["-v", &format!("{}:/work/test_files:ro", test_files_abs.display())])
            .args(["--workdir", CONTAINER_WORKDIR])
            .arg(image)
            .args(["sleep", "infinity"])
            .stdout(Stdio::piped())
            .output()
            .await
            .context("spawning docker run")?;
        if !out.status.success() {
            return Err(anyhow!("docker run failed: {}", String::from_utf8_lossy(&out.stderr)));
        }
        let container_id = String::from_utf8(out.stdout)?.trim().to_string();
        if container_id.is_empty() {
            return Err(anyhow!("empty container id from docker run"));
        }
        let sandbox = Self { container_id };
        sandbox.exec(&["git", "reset", "--hard", base_commit]).await?;
        sandbox.exec(&["bash", "-c", "cp -r /work/test_files/. /work/ladybird/"]).await?;
        Ok(sandbox)
    }

    /// Runs a command inside the container, returning (stdout, stderr).
    pub async fn exec(&self, argv: &[&str]) -> Result<(String, String)> {
        let mut cmd = Command::new("docker");
        cmd.arg("exec").arg(&self.container_id);
        for a in argv {
            cmd.arg(a);
        }
        let out = cmd.output().await.context("docker exec")?;
        let stdout = String::from_utf8_lossy(&out.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
        if !out.status.success() {
            return Err(anyhow!(
                "docker exec failed (status {:?}): {}",
                out.status.code(),
                stderr
            ));
        }
        Ok((stdout, stderr))
    }

    /// Container id (for diagnostics).
    pub fn container_id(&self) -> &str {
        &self.container_id
    }
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
