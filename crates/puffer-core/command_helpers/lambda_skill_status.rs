use puffer_resources::{LoadedResources, SkillSpec};
use std::path::Path;

/// Summarizes verified Lambda Skill readiness for user-facing surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct LambdaSkillStatus {
    pub(crate) name: String,
    pub(crate) ready: bool,
    pub(crate) gate_source: Option<String>,
    pub(crate) model_invocable: bool,
    pub(crate) model_invocation_disabled: bool,
    pub(crate) allowed_tools: Vec<String>,
    pub(crate) failure_reason: Option<String>,
}

impl LambdaSkillStatus {
    /// Renders the gate readiness status for compact panels.
    pub(crate) fn readiness_label(&self) -> String {
        match (self.ready, self.gate_source.as_deref()) {
            (true, Some(source)) => format!("gate-ready via {source}"),
            (true, None) => "gate-ready".to_string(),
            (false, _) => format!(
                "not gate-ready: {}",
                self.failure_reason
                    .as_deref()
                    .unwrap_or("missing Lambda Skill gate config")
            ),
        }
    }

    /// Renders whether the model may select this skill automatically.
    pub(crate) fn model_invocation_label(&self) -> &'static str {
        if self.model_invocable {
            "model-invocable"
        } else if self.model_invocation_disabled {
            "model invocation disabled"
        } else {
            "model invocation blocked"
        }
    }

    /// Renders the configured concrete tool scope.
    pub(crate) fn allowed_tools_label(&self) -> String {
        if self.allowed_tools.is_empty() {
            return "allowed tools <missing>".to_string();
        }
        format!("allowed tools {}", self.allowed_tools.join(", "))
    }
}

/// Returns one status value for a Lambda-verified skill.
pub(crate) fn lambda_skill_status(skill: &SkillSpec) -> Option<LambdaSkillStatus> {
    let verification = skill
        .verification
        .as_ref()
        .filter(|verification| verification.system == "lambda-skill")?;
    let allowed_tools = skill.allowed_tools.clone();
    let readiness = lambda_skill_readiness(skill, verification);
    Some(LambdaSkillStatus {
        name: skill.name.clone(),
        ready: readiness.failure_reason.is_none(),
        gate_source: readiness.gate_source,
        model_invocable: !skill.disable_model_invocation && readiness.failure_reason.is_none(),
        model_invocation_disabled: skill.disable_model_invocation,
        allowed_tools,
        failure_reason: readiness.failure_reason,
    })
}

/// Returns sorted status values for all loaded Lambda-verified skills.
pub(crate) fn lambda_skill_statuses(resources: &LoadedResources) -> Vec<LambdaSkillStatus> {
    let mut statuses = resources
        .skills
        .iter()
        .filter_map(|skill| lambda_skill_status(&skill.value))
        .collect::<Vec<_>>();
    statuses.sort_by(|left, right| left.name.cmp(&right.name));
    statuses
}

struct LambdaSkillReadiness {
    gate_source: Option<String>,
    failure_reason: Option<String>,
}

fn lambda_skill_readiness(
    skill: &SkillSpec,
    verification: &puffer_resources::SkillVerificationSpec,
) -> LambdaSkillReadiness {
    if skill.allowed_tools.is_empty() {
        return not_ready("missing allowed_tools");
    }
    if let Some(path) = verification.host_catalogue_path.as_deref() {
        let trimmed = path.trim();
        if trimmed.is_empty() {
            return not_ready("empty host catalogue path");
        }
        if !Path::new(trimmed).is_file() {
            return not_ready(format!("host catalogue not found at {trimmed}"));
        }
        return ready("host catalogue");
    }
    if let Some(compiler) = verification.compiler_path.as_deref() {
        let compiler = compiler.trim();
        if compiler.is_empty() {
            return not_ready("empty compiler path");
        }
        if !Path::new(compiler).is_file() {
            return not_ready(format!("compiler not found at {compiler}"));
        }
        let Some(source) = verification.source_path.as_deref() else {
            return not_ready("missing formal source path");
        };
        let source = source.trim();
        if source.is_empty() {
            return not_ready("empty formal source path");
        }
        if !Path::new(source).is_file() {
            return not_ready(format!("formal source not found at {source}"));
        }
        return ready("compiler");
    }
    not_ready("missing host_catalogue_subpath or compiler_path")
}

fn ready(source: &str) -> LambdaSkillReadiness {
    LambdaSkillReadiness {
        gate_source: Some(source.to_string()),
        failure_reason: None,
    }
}

fn not_ready(reason: impl Into<String>) -> LambdaSkillReadiness {
    LambdaSkillReadiness {
        gate_source: None,
        failure_reason: Some(reason.into()),
    }
}
