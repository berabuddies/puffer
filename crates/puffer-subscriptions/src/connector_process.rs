//! Subprocess bridge for connector templates that expose the typed protocol.

use crate::catalog::ConnectorTemplate;
use crate::protocol::{ConnectorActionRequest, ConnectorActionResponse};
use anyhow::{Context, Result};
use serde_json::Value;
use std::process::Stdio;
use std::time::Duration;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

/// Runs a connector template's `auth-ok` operation.
pub async fn run_auth_ok(
    template: &ConnectorTemplate,
    connection_slug: &str,
    timeout: Duration,
) -> Result<Option<bool>> {
    let Some(argv) = template.command_argv() else {
        return Ok(None);
    };
    let output = run_connector_command(
        argv,
        ["auth-ok", connection_slug].into_iter(),
        None,
        timeout,
    )
    .await
    .with_context(|| {
        format!(
            "run connector `{}` auth-ok for `{connection_slug}`",
            template.slug
        )
    })?;
    parse_auth_ok(&output).map(Some)
}

/// Runs a connector template's `act` operation.
pub async fn run_action(
    template: &ConnectorTemplate,
    request: &ConnectorActionRequest,
    timeout: Duration,
) -> Result<Option<ConnectorActionResponse>> {
    let Some(argv) = template.command_argv() else {
        return Ok(None);
    };
    let input = serde_json::to_vec(&request.input)?;
    let output = run_connector_command(
        argv,
        ["act", request.connection.as_str(), request.action.as_str()].into_iter(),
        Some(input),
        timeout,
    )
    .await
    .with_context(|| {
        format!(
            "run connector `{}` action `{}` for `{}`",
            template.slug, request.action, request.connection
        )
    })?;
    parse_action_response(&output, &request.action).map(Some)
}

async fn run_connector_command<'a, I>(
    argv: &[String],
    extra_args: I,
    stdin_json: Option<Vec<u8>>,
    timeout: Duration,
) -> Result<String>
where
    I: IntoIterator<Item = &'a str>,
{
    let Some((program, fixed_args)) = argv.split_first() else {
        anyhow::bail!("connector command is empty");
    };
    let mut command = Command::new(puffer_subscriber_runtime::resolve_manifest_program(program));
    command
        .args(fixed_args)
        .args(extra_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);
    let mut child = command
        .spawn()
        .with_context(|| format!("spawn connector command `{program}`"))?;
    if let Some(input) = stdin_json {
        let Some(mut stdin) = child.stdin.take() else {
            anyhow::bail!("connector stdin was not available");
        };
        stdin.write_all(&input).await?;
        stdin.write_all(b"\n").await?;
        stdin.shutdown().await?;
    }
    let output = tokio::time::timeout(timeout, child.wait_with_output())
        .await
        .map_err(|_| anyhow::anyhow!("connector command timed out after {:?}", timeout))?
        .context("wait for connector command")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        anyhow::bail!(
            "connector command exited with {}: {}",
            output.status,
            stderr.trim()
        );
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn parse_auth_ok(stdout: &str) -> Result<bool> {
    if stdout.eq_ignore_ascii_case("true") {
        return Ok(true);
    }
    if stdout.eq_ignore_ascii_case("false") {
        return Ok(false);
    }
    let value: Value = serde_json::from_str(stdout).context("parse auth-ok JSON")?;
    if let Some(ok) = value.get("ok").and_then(Value::as_bool) {
        return Ok(ok);
    }
    if let Some(success) = value.get("success").and_then(Value::as_bool) {
        return Ok(success);
    }
    anyhow::bail!("auth-ok output must be boolean or contain `ok`")
}

fn parse_action_response(stdout: &str, action: &str) -> Result<ConnectorActionResponse> {
    if stdout.trim().is_empty() {
        return Ok(ConnectorActionResponse {
            success: true,
            summary: format!("connector action `{action}` completed"),
            output: Value::Null,
            retryable: false,
        });
    }
    let output: Value = serde_json::from_str(stdout).context("parse connector action JSON")?;
    if let Some(success) = output.get("success").and_then(Value::as_bool) {
        let summary = output
            .get("summary")
            .or_else(|| output.get("error"))
            .and_then(Value::as_str)
            .filter(|summary| !summary.trim().is_empty())
            .map(ToString::to_string)
            .unwrap_or_else(|| {
                if success {
                    format!("connector action `{action}` completed")
                } else {
                    format!("connector action `{action}` failed")
                }
            });
        let response_output = output.get("output").cloned().unwrap_or(Value::Null);
        let retryable = output
            .get("retryable")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        return Ok(ConnectorActionResponse {
            success,
            summary,
            output: response_output,
            retryable,
        });
    }
    Ok(ConnectorActionResponse {
        success: true,
        summary: format!("connector action `{action}` completed"),
        output,
        retryable: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{
        ConnectorActionDefinition, ConnectorPermissionDefinition, ConnectorTemplate,
    };
    use std::collections::BTreeMap;

    fn template(script: &std::path::Path) -> ConnectorTemplate {
        ConnectorTemplate {
            slug: "demo".into(),
            description: "demo".into(),
            skill: "demo".into(),
            binary: script.display().to_string(),
            command: vec!["sh".into(), script.display().to_string()],
            requires_auth: true,
            can_subscribe: true,
            can_proxy_agent: false,
            subscriber: None,
            output_schema: Value::Null,
            actions: BTreeMap::from([(
                "send".into(),
                ConnectorActionDefinition {
                    slug: "send".into(),
                    description: "send".into(),
                    input_schema: Value::Null,
                    output_schema: Value::Null,
                    permission: ConnectorPermissionDefinition {
                        category: "external_message_send".into(),
                        summary: "send".into(),
                        external_side_effect: true,
                    },
                },
            )]),
        }
    }

    #[tokio::test]
    async fn runs_auth_ok_and_action_commands() {
        let temp = tempfile::tempdir().unwrap();
        let script = temp.path().join("connector.sh");
        std::fs::write(
            &script,
            r#"case "$1" in
  auth-ok) printf '{"ok":true}\n' ;;
  act) cat >/dev/null; printf '{"success":true,"summary":"sent","output":{"id":"1"}}\n' ;;
  *) exit 2 ;;
esac
"#,
        )
        .unwrap();
        let template = template(&script);

        assert_eq!(
            run_auth_ok(&template, "conn", Duration::from_secs(2))
                .await
                .unwrap(),
            Some(true)
        );
        let response = run_action(
            &template,
            &ConnectorActionRequest {
                connection: "conn".into(),
                action: "send".into(),
                input: serde_json::json!({"message":"gm"}),
                idempotency_key: None,
            },
            Duration::from_secs(2),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(response.success);
        assert_eq!(response.summary, "sent");
    }

    #[test]
    fn action_response_preserves_failure_without_summary() {
        let response = parse_action_response(
            r#"{"success":false,"error":"permission denied","retryable":true}"#,
            "send",
        )
        .unwrap();

        assert!(!response.success);
        assert_eq!(response.summary, "permission denied");
        assert!(response.retryable);
        assert_eq!(response.output, Value::Null);
    }
}
