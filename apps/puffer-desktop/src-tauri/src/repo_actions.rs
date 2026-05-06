use crate::dtos::{RepoActionResultDto, RepoPullRequestDto, RepoStatusDto};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Deserialize)]
struct GhPullRequestJson {
    number: u64,
    title: String,
    url: String,
    state: String,
    #[serde(rename = "isDraft")]
    is_draft: bool,
    #[serde(rename = "mergeStateStatus")]
    merge_state_status: Option<String>,
    #[serde(rename = "headRefName")]
    head_ref_name: Option<String>,
    #[serde(rename = "baseRefName")]
    base_ref_name: Option<String>,
}

/// Loads git and GitHub readiness state for one session working directory.
pub(crate) fn repo_status(session_id: &str, cwd: &Path) -> RepoStatusDto {
    let cwd_text = cwd.display().to_string();
    let repo_root = git_output(cwd, &["rev-parse", "--show-toplevel"]).ok();
    let branch = git_output(cwd, &["rev-parse", "--abbrev-ref", "HEAD"]).ok();
    let head_sha = git_output(cwd, &["rev-parse", "HEAD"]).ok();
    let status_text = git_output(cwd, &["status", "--short"]).unwrap_or_default();
    let status_lines = status_text
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let has_gh = command_exists("gh");
    let gh_authenticated = has_gh
        && Command::new("gh")
            .args(["auth", "status"])
            .current_dir(cwd)
            .output()
            .map(|output| output.status.success())
            .unwrap_or(false);
    let open_pull_request = if has_gh && gh_authenticated {
        repo_root
            .as_ref()
            .map(PathBuf::from)
            .as_deref()
            .and_then(|root| current_pull_request(root).ok().flatten())
    } else {
        None
    };

    let (can_create_pull_request, create_pull_request_reason) = create_pull_request_state(
        repo_root.as_deref(),
        has_gh,
        gh_authenticated,
        &open_pull_request,
    );
    let (can_merge_pull_request, merge_pull_request_reason) = merge_pull_request_state(
        repo_root.as_deref(),
        has_gh,
        gh_authenticated,
        &open_pull_request,
    );

    let mut warnings = Vec::new();
    if repo_root.is_none() {
        warnings.push("Current session is not inside a git repository.".to_string());
    }
    if !has_gh {
        warnings.push("GitHub CLI (`gh`) is not installed.".to_string());
    } else if !gh_authenticated {
        warnings.push("GitHub CLI is not authenticated.".to_string());
    }

    RepoStatusDto {
        session_id: session_id.to_string(),
        cwd: cwd_text,
        repo_root,
        branch,
        head_sha,
        is_clean: status_lines.is_empty(),
        status_lines,
        has_gh,
        gh_authenticated,
        can_create_pull_request,
        can_merge_pull_request,
        create_pull_request_reason,
        merge_pull_request_reason,
        open_pull_request,
        warnings,
    }
}

/// Creates a pull request for the session repository with optional title and body overrides.
pub(crate) fn create_pull_request(
    session_id: &str,
    cwd: &Path,
    title: Option<String>,
    body: Option<String>,
) -> RepoActionResultDto {
    let action = "create_pull_request".to_string();
    let current_status = repo_status(session_id, cwd);
    let Some(repo_root) = current_status.repo_root.as_deref().map(PathBuf::from) else {
        return failure_result(
            action,
            "Current session is not inside a git repository.",
            current_status,
        );
    };
    if !current_status.has_gh {
        return failure_result(
            action,
            "GitHub CLI (`gh`) is not installed.",
            current_status,
        );
    }
    if !current_status.gh_authenticated {
        return failure_result(action, "GitHub CLI is not authenticated.", current_status);
    }
    if !current_status.can_create_pull_request {
        let reason = current_status
            .create_pull_request_reason
            .clone()
            .unwrap_or_else(|| "Pull request creation is not currently available.".to_string());
        return failure_result(action, &reason, current_status);
    }

    let mut command = Command::new("gh");
    command.arg("pr").arg("create");
    if let Some(title) = title {
        command.arg("--title").arg(title);
        command.arg("--body").arg(body.unwrap_or_default());
    } else if let Some(body) = body {
        command.arg("--fill");
        command.arg("--body").arg(body);
    } else {
        command.arg("--fill");
    }
    command.current_dir(&repo_root);

    match command.output() {
        Ok(output) if output.status.success() => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: true,
                action,
                message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Ok(output) => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: false,
                action,
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Err(error) => failure_result(
            action,
            &format!("failed to execute gh pr create: {error}"),
            current_status,
        ),
    }
}

/// Merges a pull request for the session repository using the requested merge method.
pub(crate) fn merge_pull_request(
    session_id: &str,
    cwd: &Path,
    pull_request_number: Option<u64>,
    merge_method: Option<String>,
) -> RepoActionResultDto {
    let action = "merge_pull_request".to_string();
    let current_status = repo_status(session_id, cwd);
    let Some(repo_root) = current_status.repo_root.as_deref().map(PathBuf::from) else {
        return failure_result(
            action,
            "Current session is not inside a git repository.",
            current_status,
        );
    };
    if !current_status.has_gh {
        return failure_result(
            action,
            "GitHub CLI (`gh`) is not installed.",
            current_status,
        );
    }
    if !current_status.gh_authenticated {
        return failure_result(action, "GitHub CLI is not authenticated.", current_status);
    }
    if !current_status.can_merge_pull_request {
        let reason = current_status
            .merge_pull_request_reason
            .clone()
            .unwrap_or_else(|| "Pull request merge is not currently available.".to_string());
        return failure_result(action, &reason, current_status);
    }

    let method = merge_method.unwrap_or_else(|| "merge".to_string());
    let mut command = Command::new("gh");
    command.arg("pr").arg("merge");
    if let Some(number) = pull_request_number {
        command.arg(number.to_string());
    }
    match method.as_str() {
        "merge" => {
            command.arg("--merge");
        }
        "squash" => {
            command.arg("--squash");
        }
        "rebase" => {
            command.arg("--rebase");
        }
        other => {
            return failure_result(
                action,
                &format!("Unsupported merge method `{other}`. Use merge, squash, or rebase."),
                current_status,
            );
        }
    }
    command.arg("--delete-branch");
    command.current_dir(&repo_root);

    match command.output() {
        Ok(output) if output.status.success() => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: true,
                action,
                message: String::from_utf8_lossy(&output.stdout).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Ok(output) => {
            let refreshed = repo_status(session_id, cwd);
            RepoActionResultDto {
                ok: false,
                action,
                message: String::from_utf8_lossy(&output.stderr).trim().to_string(),
                pull_request: refreshed.open_pull_request.clone(),
                repo_status: refreshed,
            }
        }
        Err(error) => failure_result(
            action,
            &format!("failed to execute gh pr merge: {error}"),
            current_status,
        ),
    }
}

fn create_pull_request_state(
    repo_root: Option<&str>,
    has_gh: bool,
    gh_authenticated: bool,
    open_pull_request: &Option<RepoPullRequestDto>,
) -> (bool, Option<String>) {
    if repo_root.is_none() {
        return (
            false,
            Some("Current session is not inside a git repository.".to_string()),
        );
    }
    if !has_gh {
        return (
            false,
            Some("GitHub CLI (`gh`) is not installed.".to_string()),
        );
    }
    if !gh_authenticated {
        return (false, Some("GitHub CLI is not authenticated.".to_string()));
    }
    if open_pull_request.is_some() {
        return (
            false,
            Some("An open pull request already exists for the current branch.".to_string()),
        );
    }
    (true, None)
}

fn merge_pull_request_state(
    repo_root: Option<&str>,
    has_gh: bool,
    gh_authenticated: bool,
    open_pull_request: &Option<RepoPullRequestDto>,
) -> (bool, Option<String>) {
    if repo_root.is_none() {
        return (
            false,
            Some("Current session is not inside a git repository.".to_string()),
        );
    }
    if !has_gh {
        return (
            false,
            Some("GitHub CLI (`gh`) is not installed.".to_string()),
        );
    }
    if !gh_authenticated {
        return (false, Some("GitHub CLI is not authenticated.".to_string()));
    }
    if open_pull_request.is_none() {
        return (
            false,
            Some("No open pull request exists for the current branch.".to_string()),
        );
    }
    (true, None)
}

fn current_pull_request(repo_root: &Path) -> Result<Option<RepoPullRequestDto>> {
    let output = Command::new("gh")
        .args([
            "pr",
            "view",
            "--json",
            "number,title,url,state,isDraft,mergeStateStatus,headRefName,baseRefName",
        ])
        .current_dir(repo_root)
        .output()
        .context("failed to execute gh pr view")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).to_ascii_lowercase();
        if stderr.contains("no pull requests found") || stderr.contains("not found") {
            return Ok(None);
        }
        return Ok(None);
    }
    let parsed: GhPullRequestJson =
        serde_json::from_slice(&output.stdout).context("failed to parse gh pr view JSON")?;
    Ok(Some(RepoPullRequestDto {
        number: parsed.number,
        title: parsed.title,
        url: parsed.url,
        state: parsed.state,
        is_draft: parsed.is_draft,
        merge_state_status: parsed.merge_state_status,
        head_ref_name: parsed.head_ref_name,
        base_ref_name: parsed.base_ref_name,
    }))
}

fn failure_result(
    action: String,
    message: &str,
    repo_status: RepoStatusDto,
) -> RepoActionResultDto {
    RepoActionResultDto {
        ok: false,
        action,
        message: message.to_string(),
        pull_request: repo_status.open_pull_request.clone(),
        repo_status,
    }
}

fn git_output(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute git {}", args.join(" ")))?;
    if !output.status.success() {
        anyhow::bail!("{}", String::from_utf8_lossy(&output.stderr).trim());
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn command_exists(command: &str) -> bool {
    Command::new(command)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}
