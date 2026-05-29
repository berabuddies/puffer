use super::lambda_skill_status::{lambda_skill_statuses, LambdaSkillStatus};
use anyhow::Result;
use puffer_resources::LoadedResources;
use std::fmt::Write as _;

#[derive(Debug)]
struct LambdaSkillDoctorSummary {
    total: usize,
    host_catalogues: usize,
    missing_gate_config: usize,
    stats_known: usize,
    tools: usize,
    actions: usize,
}

#[derive(Debug)]
pub(crate) struct LambdaSkillDoctorWarning {
    pub(crate) summary: String,
    pub(crate) detail: String,
}

/// Appends the `/doctor` Lambda Skill resource summary lines.
pub(crate) fn append_lambda_skill_section(
    text: &mut String,
    resources: &LoadedResources,
) -> Result<()> {
    let Some(summary) = collect_lambda_skill_summary(resources) else {
        writeln!(text, "- lambda_skills=0")?;
        return Ok(());
    };
    writeln!(
        text,
        "- lambda_skills={} precompiled_catalogues={} missing_gate_config={} stats_known={} tools={} actions={}",
        summary.total,
        summary.host_catalogues,
        summary.missing_gate_config,
        summary.stats_known,
        summary.tools,
        summary.actions
    )?;
    for status in lambda_skill_statuses(resources) {
        writeln!(text, "  - {}", render_lambda_skill_status_line(&status))?;
    }
    Ok(())
}

/// Renders Lambda Skill status lines for the non-interactive CLI doctor.
pub(crate) fn render_lambda_skill_doctor_status(resources: &LoadedResources) -> String {
    let Some(summary) = collect_lambda_skill_summary(resources) else {
        return "lambda_skills=0".to_string();
    };
    let mut text = String::new();
    let _ = writeln!(
        &mut text,
        "lambda_skills={} precompiled_catalogues={} missing_gate_config={} stats_known={} tools={} actions={}",
        summary.total,
        summary.host_catalogues,
        summary.missing_gate_config,
        summary.stats_known,
        summary.tools,
        summary.actions
    );
    for status in lambda_skill_statuses(resources) {
        let _ = writeln!(
            &mut text,
            "lambda_skill {}",
            render_lambda_skill_status_line(&status)
        );
    }
    text.trim_end().to_string()
}

/// Returns Lambda Skill warnings shared by `/doctor` and `puffer doctor`.
pub(crate) fn lambda_skill_doctor_warnings(
    resources: &LoadedResources,
) -> Vec<LambdaSkillDoctorWarning> {
    let Some(summary) = collect_lambda_skill_summary(resources) else {
        return Vec::new();
    };
    let status_warnings = lambda_skill_statuses(resources)
        .into_iter()
        .filter(|status| !status.ready)
        .map(|status| LambdaSkillDoctorWarning {
            summary: format!("Lambda Skill `{}` is not gate-ready", status.name),
            detail: status
                .failure_reason
                .unwrap_or_else(|| "missing Lambda Skill gate config".to_string()),
        })
        .collect::<Vec<_>>();
    if !status_warnings.is_empty() {
        return status_warnings;
    }
    if summary.missing_gate_config > 0 {
        return vec![LambdaSkillDoctorWarning {
            summary: format!(
                "{} Lambda Skill(s) lack precompiled host catalogue config for strict gating",
                summary.missing_gate_config
            ),
            detail: "Set host_catalogue_subpath in the lambda_skill_libraries manifest."
                .to_string(),
        }];
    }
    Vec::new()
}

fn render_lambda_skill_status_line(status: &LambdaSkillStatus) -> String {
    format!(
        "{}: {}; {}; {}; {}",
        status.name,
        status.readiness_label(),
        status.model_invocation_label(),
        status.allowed_tools_label(),
        status.approval_label()
    )
}

fn collect_lambda_skill_summary(resources: &LoadedResources) -> Option<LambdaSkillDoctorSummary> {
    let mut total = 0;
    let mut host_catalogues = 0;
    let mut missing_gate_config = 0;
    let mut stats_known = 0;
    let mut tools = 0;
    let mut actions = 0;

    for skill in &resources.skills {
        let Some(verification) = skill.value.verification.as_ref() else {
            continue;
        };
        if verification.system != "lambda-skill" {
            continue;
        }
        total += 1;
        if verification.host_catalogue_path.is_some() {
            host_catalogues += 1;
        } else {
            missing_gate_config += 1;
        }
        if verification.tools.is_some() || verification.actions.is_some() {
            stats_known += 1;
        }
        tools += verification.tools.unwrap_or(0);
        actions += verification.actions.unwrap_or(0);
    }

    if total == 0 {
        return None;
    }

    Some(LambdaSkillDoctorSummary {
        total,
        host_catalogues,
        missing_gate_config,
        stats_known,
        tools,
        actions,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

    #[test]
    fn render_lambda_status_reports_missing_gate_config() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-demo".to_string(),
                    allowed_tools: vec!["Read".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: None,
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(2),
                        actions: Some(3),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains(
            "lambda_skills=1 precompiled_catalogues=0 missing_gate_config=1 stats_known=1 tools=2 actions=3"
        ));
        assert!(status.contains(
            "lambda_skill verified-demo: not gate-ready: verified Lambda Skill requires a precompiled host catalogue; set host_catalogue_subpath in the lambda_skill_libraries manifest; model invocation blocked; allowed tools Read, LambdaHostCall, LambdaInternal; verified tool approval skipped"
        ));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].summary.contains("verified-demo"));
        assert!(warnings[0]
            .detail
            .contains("requires a precompiled host catalogue"));
    }

    #[test]
    fn render_lambda_status_lists_gate_ready_skill_details() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let host_path = temp.path().join("host.json");
        std::fs::write(&source_path, "skill source").unwrap();
        std::fs::write(&host_path, r#"{"effects":[],"domains":[],"tools":[]}"#).unwrap();
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-ready".to_string(),
                    allowed_tools: vec!["Read".to_string(), "ToolSearch".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: Some(host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(2),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains("lambda_skills=1 precompiled_catalogues=1"));
        assert!(status.contains(
            "lambda_skill verified-ready: gate-ready via host catalogue; model-invocable; allowed tools Read, ToolSearch, LambdaHostCall, LambdaInternal; verified tool approval skipped"
        ));
        assert!(warnings.is_empty());
    }

    #[test]
    fn render_lambda_status_rejects_invalid_host_catalogue() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let host_path = temp.path().join("host.json");
        std::fs::write(&source_path, "skill source").unwrap();
        std::fs::write(&host_path, "not-json").unwrap();
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-broken".to_string(),
                    allowed_tools: vec!["Read".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: Some(host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(2),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains(
            "lambda_skill verified-broken: not gate-ready: failed to parse host catalogue"
        ));
        assert!(status.contains(
            "model invocation blocked; allowed tools Read, LambdaHostCall, LambdaInternal"
        ));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .detail
            .contains("failed to parse host catalogue"));
    }

    #[test]
    fn render_lambda_status_reports_missing_host_tool_binding_detail() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let host_path = temp.path().join("host.json");
        std::fs::write(&source_path, "skill source").unwrap();
        std::fs::write(
            &host_path,
            r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","effects":[]}]}"#,
        )
        .unwrap();
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-unbound".to_string(),
                    allowed_tools: vec!["Read".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: Some(host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(1),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains("Lambda Skill host tool formal_search lacks"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .detail
            .contains("Lambda Skill host tool formal_search lacks"));
    }

    #[test]
    fn render_lambda_status_reports_missing_concrete_input_contract_detail() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let host_path = temp.path().join("host.json");
        std::fs::write(&source_path, "skill source").unwrap();
        std::fs::write(
            &host_path,
            r#"{"effects":[],"domains":[],"tools":[{"name":"formal_search","effects":[],"concreteTools":["ToolSearch"],"params":[{"name":"query","ty":"str"}]}]}"#,
        )
        .unwrap();
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-uncontracted".to_string(),
                    allowed_tools: vec!["ToolSearch".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: Some(host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(1),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains("lacks a concrete input contract"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .detail
            .contains("lacks a concrete input contract"));
    }

    #[test]
    fn render_lambda_status_blocks_malformed_refinement_predicates() {
        let temp = tempfile::tempdir().unwrap();
        let source_path = temp.path().join("skill.lskill");
        let host_path = temp.path().join("host.json");
        std::fs::write(&source_path, "skill source").unwrap();
        std::fs::write(
            &host_path,
            r#"{"effects":[],"domains":[],"tools":[{"name":"custom_fetch","effects":[],"concreteTools":["ToolSearch"],"concreteInputContracts":{"ToolSearch":{"query":{"$arg":"id"}}},"params":[{"name":"id","ty":"str{host_custom_rule id}"}]}]}"#,
        )
        .unwrap();
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-unsupported".to_string(),
                    allowed_tools: vec!["ToolSearch".to_string()],
                    verification: Some(SkillVerificationSpec {
                        system: "lambda-skill".to_string(),
                        source_path: Some(source_path.display().to_string()),
                        generated_path: Some(
                            temp.path()
                                .join("out/GENERATED.SKILL.md")
                                .display()
                                .to_string(),
                        ),
                        host_catalogue_path: Some(host_path.display().to_string()),
                        compiler_path: None,
                        host_tool_bindings: Default::default(),
                        require_approval: false,
                        tools: Some(1),
                        actions: Some(1),
                    }),
                    ..SkillSpec::default()
                },
                source_info: SourceInfo {
                    path: source_path,
                    kind: SourceKind::Workspace,
                },
            }],
            ..LoadedResources::default()
        };

        let status = render_lambda_skill_doctor_status(&resources);
        let warnings = lambda_skill_doctor_warnings(&resources);

        assert!(status.contains("not gate-ready"));
        assert!(status.contains("unsupported runtime refinement host_custom_rule id"));
        assert!(status.contains("model invocation blocked; allowed tools ToolSearch"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0]
            .detail
            .contains("unsupported runtime refinement host_custom_rule id"));
    }
}
