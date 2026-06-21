use crate::AppState;
use anyhow::{Context, Result};
use serde_json::Value;
use std::fs::OpenOptions;
use std::path::Path;
use std::process::{Command, Stdio};

use super::store::{
    detect_powershell_binary, load_store, now_ms, save_store, task_output_path, tasks_path,
    PowerShellInput, StoredTask, TaskStore,
};
use super::task_runtime::wait_for_child_output;

/// Executes the live `PowerShell` workflow tool.
pub fn execute_powershell(state: &mut AppState, cwd: &Path, input: Value) -> Result<String> {
    let parsed: PowerShellInput =
        serde_json::from_value(input).context("invalid PowerShell input")?;
    let shell = detect_powershell_binary()?;
    if parsed.run_in_background {
        let tp = tasks_path(state.session.cwd.as_path(), &state.session.id);
        let mut tasks = load_store::<TaskStore>(&tp)?;
        let task_id = super::store::next_task_id(&tasks.tasks);
        let output_file = task_output_path(state.session.cwd.as_path(), &task_id)?;
        let stdout = OpenOptions::new()
            .create(true)
            .truncate(true)
            .write(true)
            .open(&output_file)
            .with_context(|| format!("failed to create {}", output_file.display()))?;
        let stderr = stdout
            .try_clone()
            .with_context(|| format!("failed to clone {}", output_file.display()))?;
        let child = Command::new(&shell)
            .args(["-NoLogo", "-Command", &parsed.command])
            .current_dir(cwd)
            .stdout(Stdio::from(stdout))
            .stderr(Stdio::from(stderr))
            .spawn()
            .with_context(|| format!("failed to start {}", shell))?;
        tasks.tasks.push(StoredTask {
            task_id: task_id.clone(),
            subject: parsed
                .description
                .clone()
                .unwrap_or_else(|| "PowerShell".to_string()),
            description: parsed.command.clone(),
            active_form: "Running PowerShell command".to_string(),
            status: "running".to_string(),
            owner: None,
            blocks: Vec::new(),
            blocked_by: Vec::new(),
            metadata: Default::default(),
            output: None,
            task_type: Some("powershell".to_string()),
            command: Some(parsed.command.clone()),
            process_id: Some(child.id()),
            output_file: Some(output_file.display().to_string()),
            received_at: None,
            expires_at: None,
            started_at_ms: Some(now_ms()),
            created_at_ms: Some(now_ms()),
            updated_at_ms: Some(now_ms()),
            exit_code: None,
            completed_via: None,
        });
        save_store(&tp, &tasks)?;
        return Ok(serde_json::to_string_pretty(&serde_json::json!({
            "stdout": "",
            "stderr": "",
            "interrupted": false,
            "backgroundTaskId": task_id,
            "outputFile": output_file.display().to_string(),
            "processId": child.id()
        }))?);
    }

    let timeout_ms = parsed.timeout.unwrap_or(120_000).clamp(1, 600_000);
    let child = Command::new(&shell)
        .args(["-NoLogo", "-Command", &parsed.command])
        .current_dir(cwd)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to execute {}", shell))?;
    let timed = wait_for_child_output(child, timeout_ms)
        .with_context(|| format!("failed to execute {}", shell))?;
    state.record_task(
        parsed
            .description
            .clone()
            .unwrap_or_else(|| "PowerShell".to_string()),
        parsed.command.clone(),
        !timed.timed_out,
    );
    Ok(serde_json::to_string_pretty(&serde_json::json!({
        "stdout": timed.stdout,
        "stderr": timed.stderr,
        "interrupted": timed.timed_out,
        "timeoutMs": timeout_ms
    }))?)
}
