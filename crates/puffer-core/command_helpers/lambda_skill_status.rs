use crate::runtime::lambda_skill_activation::{
    allowed_tools_for_verified_skill, gate_for_verified_skill_activation,
};
use puffer_resources::{LoadedResources, SkillSpec};

/// Summarizes verified Lambda Skill readiness for user-facing surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LambdaSkillStatus {
    pub name: String,
    pub ready: bool,
    pub gate_source: Option<String>,
    pub model_invocable: bool,
    pub model_invocation_disabled: bool,
    pub allowed_tools: Vec<String>,
    pub require_approval: bool,
    pub failure_reason: Option<String>,
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

    /// Renders whether verified concrete calls still require user approval.
    pub(crate) fn approval_label(&self) -> &'static str {
        if self.require_approval {
            "verified tool approval required"
        } else {
            "verified tool approval skipped"
        }
    }
}

/// Returns one status value for a Lambda-verified skill.
pub fn lambda_skill_status(skill: &SkillSpec) -> Option<LambdaSkillStatus> {
    let verification = skill
        .verification
        .as_ref()
        .filter(|verification| verification.system == "lambda-skill")?;
    let allowed_tools =
        allowed_tools_for_verified_skill(skill).unwrap_or_else(|_| skill.allowed_tools.clone());
    let readiness = lambda_skill_readiness(skill, verification);
    Some(LambdaSkillStatus {
        name: skill.name.clone(),
        ready: readiness.failure_reason.is_none(),
        gate_source: readiness.gate_source,
        model_invocable: !skill.disable_model_invocation && readiness.failure_reason.is_none(),
        model_invocation_disabled: skill.disable_model_invocation,
        allowed_tools,
        require_approval: verification.require_approval,
        failure_reason: readiness.failure_reason,
    })
}

/// Returns sorted status values for all loaded Lambda-verified skills.
pub fn lambda_skill_statuses(resources: &LoadedResources) -> Vec<LambdaSkillStatus> {
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
    _verification: &puffer_resources::SkillVerificationSpec,
) -> LambdaSkillReadiness {
    match gate_for_verified_skill_activation(skill) {
        Ok(Some(_)) => {
            let gate_source = skill
                .verification
                .as_ref()
                .and_then(|verification| verification.host_catalogue_path.as_ref())
                .map(|_| "host catalogue")
                .unwrap_or("host catalogue");
            ready(gate_source)
        }
        Ok(None) => not_ready("missing precompiled host catalogue"),
        Err(error) => not_ready(format!("{error:#}")),
    }
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
