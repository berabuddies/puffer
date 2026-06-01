use super::{
    lambda_gate_for_skill_command, render_skills_config_panel, render_skills_panel,
    render_svg_qr_data_uri, skill_allowed_tools_for_side_turn,
};
use base64::{engine::general_purpose::STANDARD as BASE64_STANDARD, Engine as _};
use puffer_resources::{
    LoadedItem, LoadedResources, SkillSpec, SkillVerificationSpec, SourceInfo, SourceKind,
};
use std::path::PathBuf;

fn loaded_skill(
    name: &str,
    description: &str,
    path: &str,
    kind: SourceKind,
) -> LoadedItem<SkillSpec> {
    LoadedItem {
        value: SkillSpec {
            name: name.to_string(),
            description: description.to_string(),
            content: "content".to_string(),
            disable_model_invocation: false,
            ..SkillSpec::default()
        },
        source_info: SourceInfo {
            path: PathBuf::from(path),
            kind,
        },
    }
}

#[test]
fn render_skills_panel_groups_skills_by_source() {
    let resources = LoadedResources {
        skills: vec![
            loaded_skill(
                "workspace-review",
                "Review workspace changes",
                "/tmp/project/.puffer/resources/skills/workspace-review/SKILL.md",
                SourceKind::Workspace,
            ),
            loaded_skill(
                "user-review",
                "Review shared changes",
                "/home/test/.puffer/resources/skills/user-review/SKILL.md",
                SourceKind::User,
            ),
            loaded_skill(
                "builtin-review",
                "Review builtin changes",
                "/app/resources/skills/builtin-review/SKILL.md",
                SourceKind::Builtin,
            ),
        ],
        ..LoadedResources::default()
    };

    let rendered = render_skills_panel(&resources);
    assert!(rendered.contains("3 skills"));
    assert!(rendered.contains("Workspace skills (/tmp/project/.puffer/resources/skills)"));
    assert!(rendered.contains("/workspace-review · ~6 description tokens"));
    assert!(rendered.contains("User skills (/home/test/.puffer/resources/skills)"));
    assert!(rendered.contains("/user-review · ~6 description tokens"));
    assert!(rendered.contains("Built-in skills (/app/resources/skills)"));
    assert!(rendered.contains("/builtin-review · ~6 description tokens"));
    assert!(rendered
        .contains("Use /skill:<name> as a compatibility alias for any user-invocable skill."));
}

#[test]
fn render_svg_qr_data_uri_embeds_svg_image_data() {
    let uri = render_svg_qr_data_uri("tg://login?token=abc").expect("qr data uri");
    let encoded = uri
        .strip_prefix("data:image/svg+xml;base64,")
        .expect("svg data uri prefix");
    let decoded = BASE64_STANDARD.decode(encoded).expect("base64 svg");
    let svg = String::from_utf8(decoded).expect("utf8 svg");
    assert!(svg.contains("<svg"));
    assert!(svg.contains("</svg>"));
}

#[test]
fn render_skills_panel_reports_missing_skills() {
    let rendered = render_skills_panel(&LoadedResources::default());
    assert!(rendered.contains("No skills found."));
    assert!(rendered.contains("~/.puffer/resources/skills/"));
    assert!(rendered.contains("/skill:<name>"));
    assert!(rendered.contains("/skills config"));
}

#[test]
fn render_skills_config_panel_shows_lambda_manifest_template() {
    let temp = tempfile::tempdir().unwrap();
    let rendered = render_skills_config_panel(temp.path());
    assert!(rendered.contains("resources/lambda_skill_libraries"));
    assert!(rendered.contains("host_catalogue_subpath: out/host.json"));
    assert!(rendered.contains("precompiled verification output"));
}

#[test]
fn render_skills_panel_marks_hidden_and_model_disabled_skills() {
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: SkillSpec {
                name: "hidden-review".to_string(),
                description: "Hidden review entry".to_string(),
                user_invocable: false,
                disable_model_invocation: true,
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("/tmp/project/.puffer/resources/skills/hidden-review/SKILL.md"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    let rendered = render_skills_panel(&resources);
    assert!(rendered.contains(
        "- hidden-review · ~5 description tokens · hidden from slash-command invocation · model invocation disabled"
    ));
}

#[test]
fn render_skills_panel_marks_verified_skills() {
    let temp = tempfile::tempdir().unwrap();
    let host_path = temp.path().join("host.json");
    std::fs::write(&host_path, r#"{"effects":[],"domains":[],"tools":[]}"#).unwrap();
    let resources = LoadedResources {
        skills: vec![LoadedItem {
            value: SkillSpec {
                name: "verified-ci".to_string(),
                description: "Fix verified CI failures".to_string(),
                allowed_tools: vec!["Read".to_string()],
                verification: Some(SkillVerificationSpec {
                    system: "lambda-skill".to_string(),
                    source_path: Some("fixtures/skills/verified-ci/skill.lskill".to_string()),
                    generated_path: Some(
                        "fixtures/skills/verified-ci/out/GENERATED.SKILL.md".to_string(),
                    ),
                    host_catalogue_path: Some(host_path.display().to_string()),
                    compiler_path: None,
                    host_tool_bindings: Default::default(),
                    require_approval: false,
                    tools: Some(10),
                    actions: Some(2),
                }),
                ..SkillSpec::default()
            },
            source_info: SourceInfo {
                path: PathBuf::from("fixtures/skills/verified-ci/skill.lskill"),
                kind: SourceKind::Workspace,
            },
        }],
        ..LoadedResources::default()
    };

    let rendered = render_skills_panel(&resources);
    assert!(rendered.contains(
        "- /verified-ci · ~6 description tokens · verified lambda-skill (10 tools, 2 actions) · gate-ready via host catalogue · model-invocable · allowed tools Read, LambdaHostCall, LambdaInternal · verified tool approval skipped"
    ));
}

#[test]
fn lambda_skill_side_turn_includes_host_bridge_when_filtered() {
    let skill = SkillSpec {
        name: "verified-ci".to_string(),
        allowed_tools: vec!["Read".to_string()],
        verification: Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: Some("fixtures/skills/verified-ci/skill.lskill".to_string()),
            generated_path: Some("fixtures/skills/verified-ci/out/GENERATED.SKILL.md".to_string()),
            host_catalogue_path: None,
            compiler_path: None,
            host_tool_bindings: Default::default(),
            require_approval: false,
            tools: Some(10),
            actions: Some(2),
        }),
        ..SkillSpec::default()
    };

    let allowed = skill_allowed_tools_for_side_turn(&skill).unwrap();

    assert_eq!(
        allowed,
        vec![
            "Read".to_string(),
            "LambdaHostCall".to_string(),
            "LambdaInternal".to_string()
        ]
    );
}

#[test]
fn lambda_skill_side_turn_rejects_empty_filter() {
    let skill = SkillSpec {
        name: "verified-ci".to_string(),
        verification: Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: Some("fixtures/skills/verified-ci/skill.lskill".to_string()),
            generated_path: Some("fixtures/skills/verified-ci/out/GENERATED.SKILL.md".to_string()),
            host_catalogue_path: None,
            compiler_path: None,
            host_tool_bindings: Default::default(),
            require_approval: false,
            tools: Some(10),
            actions: Some(2),
        }),
        ..SkillSpec::default()
    };

    let error = skill_allowed_tools_for_side_turn(&skill)
        .unwrap_err()
        .to_string();

    assert!(error.contains("requires non-empty allowed_tools"));
}

#[test]
fn lambda_skill_command_rejects_prompt_only_verified_skill() {
    let skill = SkillSpec {
        name: "verified-ci".to_string(),
        verification: Some(SkillVerificationSpec {
            system: "lambda-skill".to_string(),
            source_path: Some("fixtures/skills/verified-ci/skill.lskill".to_string()),
            generated_path: Some("fixtures/skills/verified-ci/out/GENERATED.SKILL.md".to_string()),
            host_catalogue_path: None,
            compiler_path: None,
            host_tool_bindings: Default::default(),
            require_approval: false,
            tools: Some(10),
            actions: Some(2),
        }),
        ..SkillSpec::default()
    };

    let error = lambda_gate_for_skill_command(&skill)
        .unwrap_err()
        .to_string();
    assert!(error.contains("verified Lambda Skill requires a precompiled host catalogue"));
}
