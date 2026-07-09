use puffer_workflow::{WorkflowRuntimeError, WorkflowRuntimeErrorKind};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AutomationRuntimeErrorContext {
    Automation,
    Request,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AutomationRuntimeFailure {
    DockerMissing,
    PortConflict,
    ImageUnavailable,
    DatabasePrepareFailed,
    TokenMissing,
    AuthFailed,
    WorkspaceInaccessible,
    IncompatibleRuntime,
    RuntimeUnreachable,
    RequestPreparationFailed,
}

pub(crate) fn public_automation_runtime_error_message(
    error: &anyhow::Error,
    context: AutomationRuntimeErrorContext,
) -> String {
    let detail = format!("{error:#}");
    public_automation_runtime_detail_message(&detail, context)
}

pub(crate) fn public_automation_runtime_error(
    error: &WorkflowRuntimeError,
    context: AutomationRuntimeErrorContext,
) -> String {
    match error.kind {
        WorkflowRuntimeErrorKind::InvalidToken => {
            "Invalid token. Check the selected automation runtime credentials, then try again."
                .to_string()
        }
        WorkflowRuntimeErrorKind::PermissionDenied => {
            "Automation runtime credentials do not have permission for this workspace.".to_string()
        }
        WorkflowRuntimeErrorKind::WorkspaceInaccessible => {
            "Automation workspace is not accessible. Check the selected workspace, then try again."
                .to_string()
        }
        WorkflowRuntimeErrorKind::RuntimeUnreachable => runtime_unreachable_message(),
        WorkflowRuntimeErrorKind::IncompatibleRuntime => {
            public_automation_runtime_detail_message(&error.message, context)
        }
        WorkflowRuntimeErrorKind::ServiceError => {
            "Automation runtime returned an error. Check the selected runtime, then try again."
                .to_string()
        }
    }
}

pub(crate) fn public_automation_runtime_detail_message(
    detail: &str,
    context: AutomationRuntimeErrorContext,
) -> String {
    match classify_automation_runtime_failure(detail) {
        AutomationRuntimeFailure::DockerMissing => {
            "Docker is required to run local automations. Install or start Docker, then try again."
                .to_string()
        }
        AutomationRuntimeFailure::PortConflict => {
            "Puffer could not update the local automation runtime because a stale container or port conflict is blocking it. Close the conflicting process, then try again."
                .to_string()
        }
        AutomationRuntimeFailure::ImageUnavailable => {
            "The local automation runtime image could not be installed or updated. Check Docker registry access, then try again."
                .to_string()
        }
        AutomationRuntimeFailure::DatabasePrepareFailed => {
            "The Puffer-managed local automation runtime database could not be prepared. Puffer needs to rebuild the local runtime data before automations can run."
                .to_string()
        }
        AutomationRuntimeFailure::TokenMissing => match context {
            AutomationRuntimeErrorContext::Automation => token_or_workspace_message(),
            AutomationRuntimeErrorContext::Request => {
                "Automation runtime token is not configured. Add credentials for the selected runtime, then try again."
                    .to_string()
            }
        },
        AutomationRuntimeFailure::AuthFailed => match context {
            AutomationRuntimeErrorContext::Automation => token_or_workspace_message(),
            AutomationRuntimeErrorContext::Request => {
                "Invalid token. Check the selected automation runtime credentials, then try again."
                    .to_string()
            }
        },
        AutomationRuntimeFailure::WorkspaceInaccessible => match context {
            AutomationRuntimeErrorContext::Automation => token_or_workspace_message(),
            AutomationRuntimeErrorContext::Request => {
                "Automation workspace is not accessible. Check the selected workspace, then try again."
                    .to_string()
            }
        },
        AutomationRuntimeFailure::IncompatibleRuntime => {
            "The Puffer-managed local automation runtime is not compatible with this Puffer build. Puffer could not update it automatically; try again after Docker is ready."
                .to_string()
        }
        AutomationRuntimeFailure::RuntimeUnreachable => runtime_unreachable_message(),
        AutomationRuntimeFailure::RequestPreparationFailed => request_preparation_message(context),
    }
}

fn classify_automation_runtime_failure(detail: &str) -> AutomationRuntimeFailure {
    let lower = detail.to_ascii_lowercase();
    if lower.contains("docker_missing") || lower.contains("docker") && lower.contains("not found") {
        return AutomationRuntimeFailure::DockerMissing;
    }
    if lower.contains("port is already allocated")
        || lower.contains("bind for")
        || lower.contains("stale local runtime container")
    {
        return AutomationRuntimeFailure::PortConflict;
    }
    if lower.contains("image_missing")
        || lower.contains("image is not installed")
        || lower.contains("could not be downloaded")
        || lower.contains("pull failed")
    {
        return AutomationRuntimeFailure::ImageUnavailable;
    }
    if lower.contains("migrate failed")
        || lower.contains("seed failed")
        || lower.contains("database migration failed")
        || lower.contains("global/pg_filenode.map")
        || lower.contains("pg-protocol")
    {
        return AutomationRuntimeFailure::DatabasePrepareFailed;
    }
    if lower.contains("token is not configured") {
        return AutomationRuntimeFailure::TokenMissing;
    }
    if lower.contains("credentials")
        || lower.contains("invalid token")
        || lower.contains("permission denied")
    {
        return AutomationRuntimeFailure::AuthFailed;
    }
    if lower.contains("workspace inaccessible")
        || lower.contains("workspace access failed")
        || lower.contains("workspace inaccessible or not found")
    {
        return AutomationRuntimeFailure::WorkspaceInaccessible;
    }
    if lower.contains("incompatible_runtime")
        || lower.contains("incompatible workflow runtime")
        || lower.contains("node definitions")
        || lower.contains("rejected local credentials")
        || lower.contains("unexpected schema")
        || lower.contains("invalid json")
    {
        return AutomationRuntimeFailure::IncompatibleRuntime;
    }
    if lower.contains("error sending request")
        || lower.contains("connection refused")
        || lower.contains("timed out")
        || lower.contains("runtime unreachable")
    {
        return AutomationRuntimeFailure::RuntimeUnreachable;
    }
    AutomationRuntimeFailure::RequestPreparationFailed
}

fn runtime_unreachable_message() -> String {
    "Automation runtime is unreachable. Check Docker or the selected runtime settings, then try again."
        .to_string()
}

fn token_or_workspace_message() -> String {
    "Automation runtime token or workspace access failed. Check the selected run location credentials, then try again."
        .to_string()
}

fn request_preparation_message(context: AutomationRuntimeErrorContext) -> String {
    match context {
        AutomationRuntimeErrorContext::Automation => {
            "Automation runtime could not prepare this automation. Check the selected run location and try again."
                .to_string()
        }
        AutomationRuntimeErrorContext::Request => {
            "Automation runtime could not prepare this request. Check the selected runtime and try again."
                .to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hides_local_migration_diagnostics() {
        let raw = r#"local workflow runtime is incompatible_runtime: docker compose run --rm migrate failed: Container puffer-workflow-runtime-postgres-1 Healthy Database migration failed: error: could not open file "global/pg_filenode.map": No such file or directory at Parser.parseErrorMessage (/app/node_modules/pg-protocol/dist/parser.js:285:98) at TCP.onStreamRead (node:internal/stream_base_commons:191:23); could not refresh agentenv/api-server:local"#;

        let message =
            public_automation_runtime_detail_message(raw, AutomationRuntimeErrorContext::Request);

        assert_eq!(
            message,
            "The Puffer-managed local automation runtime database could not be prepared. Puffer needs to rebuild the local runtime data before automations can run."
        );
        assert!(!message.contains("global/pg_filenode.map"));
        assert!(!message.contains("Parser.parseErrorMessage"));
        assert!(!message.contains("node_modules"));
        assert!(!message.contains("agentenv/api-server"));
    }

    #[test]
    fn maps_context_specific_fallbacks() {
        let detail = "unexpected backend failure with /v1/workflows";

        assert_eq!(
            public_automation_runtime_detail_message(
                detail,
                AutomationRuntimeErrorContext::Request
            ),
            "Automation runtime could not prepare this request. Check the selected runtime and try again."
        );
        assert_eq!(
            public_automation_runtime_detail_message(
                detail,
                AutomationRuntimeErrorContext::Automation
            ),
            "Automation runtime could not prepare this automation. Check the selected run location and try again."
        );
    }

    #[test]
    fn maps_context_specific_credential_failures() {
        let detail = "workflow runtime permission denied";

        assert_eq!(
            public_automation_runtime_detail_message(
                detail,
                AutomationRuntimeErrorContext::Request
            ),
            "Invalid token. Check the selected automation runtime credentials, then try again."
        );
        assert_eq!(
            public_automation_runtime_detail_message(
                detail,
                AutomationRuntimeErrorContext::Automation
            ),
            "Automation runtime token or workspace access failed. Check the selected run location credentials, then try again."
        );
    }
}
