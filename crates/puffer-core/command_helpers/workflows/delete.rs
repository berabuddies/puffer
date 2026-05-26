use crate::subscription_manager;
use anyhow::{Context, Result};
use std::fmt::Write as _;

/// Deletes one workflow binding from the terminal workflow command surface.
pub(super) fn delete_workflow_binding(args: &str) -> Result<String> {
    let slug = parse_delete_args(args)?;
    let manager = subscription_manager()?;
    manager.store().delete(slug)?;
    manager.refresh_connection_consumers()?;

    let mut out = String::new();
    let _ = writeln!(out, "Deleted workflow action `{slug}`.");
    let _ = writeln!(
        out,
        "Run /workflows actions to inspect remaining workflow actions."
    );
    Ok(out)
}

fn parse_delete_args(args: &str) -> Result<&str> {
    let slug = args
        .split_whitespace()
        .next()
        .context("Usage: /workflows delete <binding-slug>")?;
    if args.split_whitespace().nth(1).is_some() {
        anyhow::bail!("Usage: /workflows delete <binding-slug>");
    }
    Ok(slug)
}

#[cfg(test)]
mod tests {
    use super::parse_delete_args;

    #[test]
    fn parses_delete_slug() {
        assert_eq!(
            parse_delete_args("append-telegram-user-hi").unwrap(),
            "append-telegram-user-hi"
        );
    }

    #[test]
    fn rejects_missing_delete_slug() {
        let error = parse_delete_args("   ").unwrap_err().to_string();

        assert!(error.contains("/workflows delete <binding-slug>"));
    }

    #[test]
    fn rejects_extra_delete_args() {
        let error = parse_delete_args("append-a extra").unwrap_err().to_string();

        assert!(error.contains("/workflows delete <binding-slug>"));
    }
}
