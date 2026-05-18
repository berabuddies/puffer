use crate::dtos::RemoteOperationDto;
use anyhow::{anyhow, Context, Result};
use base64::prelude::*;
use serde::de::DeserializeOwned;
use std::fs;
use std::path::PathBuf;
use std::process::Command;
use uuid::Uuid;

const REMOTE_FILE_WRITE_MAX_BYTES: usize = 5 * 1024 * 1024;

/// Executes a `puffer desktop-api ...` command on a remote host over SSH and parses the JSON.
#[allow(dead_code)]
pub(crate) fn run_remote_json<T: DeserializeOwned>(
    target: &str,
    remote_cwd: Option<&str>,
    remote_password: Option<&str>,
    args: &[String],
) -> Result<T> {
    let remote_command = build_remote_command(remote_cwd, args);
    let mut cleanup_path = None;
    let mut command = Command::new("ssh");
    command.args([
        "-o",
        "BatchMode=no",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=15",
        target,
        "bash",
        "-lc",
        &remote_command,
    ]);
    if let Some(password) = remote_password.filter(|value| !value.trim().is_empty()) {
        let askpass = write_askpass_script()?;
        cleanup_path = Some(askpass.clone());
        command.env("SSH_ASKPASS", &askpass);
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", "puffer-desktop");
        command.env("PUFFER_SSH_PASSWORD", password);
    }

    let output = command
        .output()
        .with_context(|| format!("failed to execute ssh command for `{target}`"))?;
    if let Some(path) = cleanup_path {
        let _ = fs::remove_file(path);
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if !stderr.is_empty() { stderr } else { stdout };
        return Err(anyhow!("remote ssh command failed: {detail}"));
    }

    serde_json::from_slice(&output.stdout).context("failed to parse remote JSON response")
}

/// Executes an arbitrary shell command on the remote host and captures stdout/stderr.
pub(crate) fn run_remote_shell(
    target: &str,
    remote_cwd: Option<&str>,
    remote_password: Option<&str>,
    shell_command: &str,
) -> Result<RemoteOperationDto> {
    let remote_command = if let Some(cwd) = remote_cwd.filter(|value| !value.trim().is_empty()) {
        format!("cd {} && {}", shell_quote(cwd), shell_command)
    } else {
        shell_command.to_string()
    };
    let mut cleanup_path = None;
    let mut command = Command::new("ssh");
    command.args([
        "-o",
        "BatchMode=no",
        "-o",
        "StrictHostKeyChecking=accept-new",
        "-o",
        "ConnectTimeout=15",
        target,
        "bash",
        "-lc",
        &remote_command,
    ]);
    if let Some(password) = remote_password.filter(|value| !value.trim().is_empty()) {
        let askpass = write_askpass_script()?;
        cleanup_path = Some(askpass.clone());
        command.env("SSH_ASKPASS", &askpass);
        command.env("SSH_ASKPASS_REQUIRE", "force");
        command.env("DISPLAY", "puffer-desktop");
        command.env("PUFFER_SSH_PASSWORD", password);
    }
    let output = command
        .output()
        .with_context(|| format!("failed to execute ssh command for `{target}`"))?;
    if let Some(path) = cleanup_path {
        let _ = fs::remove_file(path);
    }
    Ok(RemoteOperationDto {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

/// Reads one text file from the remote host, honoring the optional remote cwd.
pub(crate) fn read_remote_file(
    target: &str,
    remote_cwd: Option<&str>,
    remote_password: Option<&str>,
    path: &str,
) -> Result<RemoteOperationDto> {
    run_remote_shell(
        target,
        remote_cwd,
        remote_password,
        &remote_read_file_command(path),
    )
}

/// Writes one file on the remote host from a base64-encoded payload.
pub(crate) fn write_remote_file(
    target: &str,
    remote_cwd: Option<&str>,
    remote_password: Option<&str>,
    path: &str,
    contents_base64: &str,
) -> Result<RemoteOperationDto> {
    validate_remote_write_payload(contents_base64)?;
    run_remote_shell(
        target,
        remote_cwd,
        remote_password,
        &remote_write_file_command(path, contents_base64),
    )
}

#[allow(dead_code)]
fn build_remote_command(remote_cwd: Option<&str>, args: &[String]) -> String {
    let mut command = String::new();
    if let Some(cwd) = remote_cwd.filter(|value| !value.trim().is_empty()) {
        command.push_str("cd ");
        command.push_str(&shell_quote(cwd));
        command.push_str(" && ");
    }
    let desktop_args = args
        .iter()
        .map(|arg| shell_quote(arg))
        .collect::<Vec<_>>()
        .join(" ");
    command.push_str("if command -v puffer >/dev/null 2>&1; then exec puffer desktop-api ");
    command.push_str(&desktop_args);
    command.push_str("; ");
    command.push_str("elif [ -x \"$HOME/.cargo/bin/puffer\" ]; then exec \"$HOME/.cargo/bin/puffer\" desktop-api ");
    command.push_str(&desktop_args);
    command.push_str("; ");
    command.push_str(
        "elif [ -x \"./target/debug/puffer\" ]; then exec \"./target/debug/puffer\" desktop-api ",
    );
    command.push_str(&desktop_args);
    command.push_str("; ");
    command.push_str("elif [ -x \"$HOME/.cargo/bin/cargo\" ]; then exec \"$HOME/.cargo/bin/cargo\" run -q -p puffer-cli -- desktop-api ");
    command.push_str(&desktop_args);
    command.push_str("; ");
    command.push_str("elif command -v cargo >/dev/null 2>&1; then exec cargo run -q -p puffer-cli -- desktop-api ");
    command.push_str(&desktop_args);
    command.push_str("; ");
    command.push_str("else echo 'remote puffer desktop-api command not found' >&2; exit 127; fi");
    command
}

pub(crate) fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', r#"'"'"'"#))
}

fn remote_read_file_command(path: &str) -> String {
    format!("cat < {}", shell_quote(path))
}

fn remote_write_file_command(path: &str, contents_base64: &str) -> String {
    format!(
        "printf %s {} | {{ base64 --decode 2>/dev/null || base64 -D; }} > {}",
        shell_quote(contents_base64),
        shell_quote(path)
    )
}

fn validate_remote_write_payload(contents_base64: &str) -> Result<()> {
    let bytes = BASE64_STANDARD
        .decode(contents_base64)
        .context("invalid base64")?;
    if bytes.len() > REMOTE_FILE_WRITE_MAX_BYTES {
        anyhow::bail!(
            "file is too large to write ({} bytes, hard limit {} bytes)",
            bytes.len(),
            REMOTE_FILE_WRITE_MAX_BYTES
        );
    }
    Ok(())
}

fn write_askpass_script() -> Result<PathBuf> {
    let path = std::env::temp_dir().join(format!("puffer-askpass-{}.sh", Uuid::new_v4().simple()));
    fs::write(&path, "#!/bin/sh\nprintf '%s' \"$PUFFER_SSH_PASSWORD\"\n")
        .with_context(|| format!("failed to write askpass helper {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut permissions = fs::metadata(&path)?.permissions();
        permissions.set_mode(0o700);
        fs::set_permissions(&path, permissions)?;
    }
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::{
        remote_read_file_command, remote_write_file_command, shell_quote,
        validate_remote_write_payload,
    };

    #[test]
    fn shell_quote_handles_embedded_single_quotes() {
        assert_eq!(shell_quote("a'b"), "'a'\"'\"'b'");
    }

    #[test]
    fn remote_read_file_command_quotes_path_without_local_io() {
        let command = remote_read_file_command("notes/weird ' file.txt");

        assert_eq!(command, "cat < 'notes/weird '\"'\"' file.txt'");
    }

    #[test]
    fn remote_write_file_command_decodes_payload_to_quoted_path() {
        let command = remote_write_file_command("notes/out.txt", "SGVsbG8=");

        assert_eq!(
            command,
            "printf %s 'SGVsbG8=' | { base64 --decode 2>/dev/null || base64 -D; } > 'notes/out.txt'"
        );
    }

    #[test]
    fn validate_remote_write_payload_rejects_invalid_base64() {
        assert!(validate_remote_write_payload("not base64!").is_err());
    }
}
