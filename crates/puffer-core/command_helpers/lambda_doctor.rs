use anyhow::Result;
use puffer_resources::LoadedResources;
use std::fmt::Write as _;
use std::path::PathBuf;

#[derive(Debug)]
struct LambdaSkillDoctorSummary {
    total: usize,
    host_catalogues: usize,
    compile_sources: usize,
    stats_known: usize,
    tools: usize,
    actions: usize,
    compiler: Option<PathBuf>,
    first_compile_source: Option<PathBuf>,
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
        "- lambda_skills={} strict_catalogues={} compile_sources={} stats_known={} tools={} actions={}",
        summary.total,
        summary.host_catalogues,
        summary.compile_sources,
        summary.stats_known,
        summary.tools,
        summary.actions
    )?;
    writeln!(
        text,
        "  compile_gate={} {}={}",
        lambda_compile_gate_label(),
        crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV,
        display_compiler(&summary)
    )?;
    if let Some(source) = summary.first_compile_source.as_ref() {
        writeln!(text, "  first_compile_source={}", source.display())?;
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
        "lambda_skills={} strict_catalogues={} compile_sources={} stats_known={} tools={} actions={}",
        summary.total,
        summary.host_catalogues,
        summary.compile_sources,
        summary.stats_known,
        summary.tools,
        summary.actions
    );
    let _ = writeln!(
        &mut text,
        "lambda_skill_compile_gate={}",
        lambda_compile_gate_label()
    );
    let _ = writeln!(
        &mut text,
        "lambda_skill_lskillc={}",
        display_compiler(&summary)
    );
    if let Some(source) = summary.first_compile_source.as_ref() {
        let _ = writeln!(
            &mut text,
            "lambda_skill_first_compile_source={}",
            source.display()
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
    if summary.compile_sources > 0
        && !crate::runtime::lambda_gate::lambda_skill_compiler_gate_enabled()
    {
        return vec![LambdaSkillDoctorWarning {
            summary: format!(
                "{} Lambda Skill(s) require compile-on-demand host catalogues for strict gating",
                summary.compile_sources
            ),
            detail: format!(
                "Set {}=compile to enforce host catalogues without vendoring generated files.",
                crate::runtime::lambda_gate::LAMBDA_SKILL_GATE_ENV
            ),
        }];
    }
    if summary.compile_sources > 0 && summary.compiler.is_none() {
        return vec![LambdaSkillDoctorWarning {
            summary: "Lambda Skill compile gate is enabled but lskillc was not found".to_string(),
            detail: format!(
                "Set {} to the Lambda Skill compiler path.",
                crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV
            ),
        }];
    }
    Vec::new()
}

fn lambda_compile_gate_label() -> &'static str {
    if crate::runtime::lambda_gate::lambda_skill_compiler_gate_enabled() {
        "enabled"
    } else {
        "disabled"
    }
}

fn display_compiler(summary: &LambdaSkillDoctorSummary) -> String {
    summary
        .compiler
        .as_ref()
        .map(|path| path.display().to_string())
        .unwrap_or_else(|| "<missing>".to_string())
}

fn collect_lambda_skill_summary(resources: &LoadedResources) -> Option<LambdaSkillDoctorSummary> {
    let mut total = 0;
    let mut host_catalogues = 0;
    let mut compile_sources = 0;
    let mut stats_known = 0;
    let mut tools = 0;
    let mut actions = 0;
    let mut first_compile_source = None;

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
        } else if let Some(source_path) = verification.source_path.as_ref() {
            compile_sources += 1;
            first_compile_source.get_or_insert_with(|| PathBuf::from(source_path));
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

    let compiler = first_compile_source
        .as_ref()
        .and_then(|source| crate::runtime::lambda_gate::resolve_lskillc_for_source(source));
    Some(LambdaSkillDoctorSummary {
        total,
        host_catalogues,
        compile_sources,
        stats_known,
        tools,
        actions,
        compiler,
        first_compile_source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_resources::{LoadedItem, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind};

    #[test]
    fn render_lambda_status_reports_prompt_only_compile_sources() {
        let _guard = crate::test_locks::env_lock()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let old_gate = std::env::var_os(crate::runtime::lambda_gate::LAMBDA_SKILL_GATE_ENV);
        let old_compiler = std::env::var_os(crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV);
        let old_path = std::env::var_os("PATH");
        std::env::remove_var(crate::runtime::lambda_gate::LAMBDA_SKILL_GATE_ENV);
        std::env::remove_var(crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV);

        let temp = tempfile::tempdir().unwrap();
        std::env::set_var("PATH", temp.path());
        let source_path = temp.path().join("skill.lskill");
        let resources = LoadedResources {
            skills: vec![LoadedItem {
                value: SkillSpec {
                    name: "verified-demo".to_string(),
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
            "lambda_skills=1 strict_catalogues=0 compile_sources=1 stats_known=1 tools=2 actions=3"
        ));
        assert!(status.contains("lambda_skill_compile_gate=disabled"));
        assert!(status.contains("lambda_skill_lskillc=<missing>"));
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].summary.contains("1 Lambda Skill(s) require"));

        match old_gate {
            Some(value) => {
                std::env::set_var(crate::runtime::lambda_gate::LAMBDA_SKILL_GATE_ENV, value)
            }
            None => std::env::remove_var(crate::runtime::lambda_gate::LAMBDA_SKILL_GATE_ENV),
        }
        match old_compiler {
            Some(value) => std::env::set_var(
                crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV,
                value,
            ),
            None => std::env::remove_var(crate::runtime::lambda_gate::LAMBDA_SKILL_COMPILER_ENV),
        }
        match old_path {
            Some(value) => std::env::set_var("PATH", value),
            None => std::env::remove_var("PATH"),
        }
    }
}
