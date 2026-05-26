use crate::subscription_manager;
use anyhow::{Context, Result};
use puffer_subscriptions::WorkflowBindingStatus;
use std::fmt::Write as _;

/// Pauses one workflow binding from the terminal workflow command surface.
pub(super) fn pause_workflow_binding(args: &str) -> Result<String> {
    toggle_workflow_binding(args, WorkflowBindingStatus::Paused, "pause")
}

/// Resumes one workflow binding from the terminal workflow command surface.
pub(super) fn resume_workflow_binding(args: &str) -> Result<String> {
    toggle_workflow_binding(args, WorkflowBindingStatus::Enabled, "resume")
}

fn toggle_workflow_binding(
    args: &str,
    status: WorkflowBindingStatus,
    command: &'static str,
) -> Result<String> {
    let slug = parse_toggle_args(args, command)?;
    let manager = subscription_manager()?;
    let binding = manager.store().set_status(slug, status)?;
    manager.refresh_connection_consumers()?;

    let verb = match status {
        WorkflowBindingStatus::Enabled => "Resumed",
        WorkflowBindingStatus::Paused => "Paused",
    };
    let mut out = String::new();
    let _ = writeln!(out, "{verb} workflow action `{}`.", binding.slug);
    let _ = writeln!(out, "status={}", status_label(binding.status));
    let _ = writeln!(out, "Run /workflows actions to inspect workflow actions.");
    Ok(out)
}

fn parse_toggle_args<'a>(args: &'a str, command: &'static str) -> Result<&'a str> {
    let usage = format!("Usage: /workflows {command} <binding-slug>");
    let slug = args.split_whitespace().next().context(usage.clone())?;
    if args.split_whitespace().nth(1).is_some() {
        anyhow::bail!(usage);
    }
    Ok(slug)
}

fn status_label(status: WorkflowBindingStatus) -> &'static str {
    match status {
        WorkflowBindingStatus::Enabled => "enabled",
        WorkflowBindingStatus::Paused => "paused",
    }
}

#[cfg(test)]
mod tests {
    use super::parse_toggle_args;

    #[test]
    fn parses_toggle_slug() {
        assert_eq!(
            parse_toggle_args("append-telegram-user-hi", "pause").unwrap(),
            "append-telegram-user-hi"
        );
    }

    #[test]
    fn rejects_missing_toggle_slug() {
        let error = parse_toggle_args("   ", "resume").unwrap_err().to_string();

        assert!(error.contains("/workflows resume <binding-slug>"));
    }

    #[test]
    fn rejects_extra_toggle_args() {
        let error = parse_toggle_args("append-a extra", "pause")
            .unwrap_err()
            .to_string();

        assert!(error.contains("/workflows pause <binding-slug>"));
    }
}
