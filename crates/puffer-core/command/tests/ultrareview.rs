use super::*;
use puffer_resources::AgentSpec;
use std::fs;
use std::path::{Path, PathBuf};

const LANE_AGENT_IDS: &[&str] = &[
    "reviewer-security",
    "reviewer-logic",
    "reviewer-duplication",
    "reviewer-editorial",
    "reviewer-architecture",
];
const PIPELINE_AGENT_IDS: &[&str] = &["reviewer-planner", "reviewer-filter"];

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn load_agent(relative_path: &str) -> AgentSpec {
    let contents = fs::read_to_string(repo_root().join(relative_path)).unwrap();
    serde_yaml::from_str(&contents).unwrap()
}

#[test]
fn ultrareview_command_registered_as_local_with_pr_hint() {
    let commands = supported_commands();
    let cmd = find_command(&commands, "ultrareview").expect("ultrareview command registered");
    assert_eq!(cmd.kind, CommandKind::Local);
    assert_eq!(cmd.argument_hint.as_deref(), Some("[pr-url-or-number]"));
    assert!(cmd.aliases.is_empty());
    assert!(!cmd.hidden);
}

#[test]
fn ultrareview_command_sits_between_theme_and_usage() {
    let commands = supported_commands();
    let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
    let theme_idx = names
        .iter()
        .position(|n| *n == "theme")
        .expect("theme present");
    let ultra_idx = names
        .iter()
        .position(|n| *n == "ultrareview")
        .expect("ultrareview present");
    let usage_idx = names
        .iter()
        .position(|n| *n == "usage")
        .expect("usage present");
    assert!(
        theme_idx < ultra_idx && ultra_idx < usage_idx,
        "expected theme < ultrareview < usage, got indices {theme_idx} {ultra_idx} {usage_idx}"
    );
}

#[test]
fn ultrareview_agents_all_load_with_expected_fields() {
    for agent in PIPELINE_AGENT_IDS.iter().chain(LANE_AGENT_IDS.iter()) {
        let path = format!("resources/agents/{agent}.yaml");
        let spec = load_agent(&path);
        assert_eq!(spec.id, *agent, "id mismatch in {path}");
        assert!(
            !spec.description.is_empty(),
            "missing description in {path}"
        );
        assert!(!spec.prompt.is_empty(), "missing prompt in {path}");
        assert_eq!(
            spec.isolation.as_deref(),
            Some("worktree"),
            "{path} must set isolation: worktree"
        );
    }
}

#[test]
fn ultrareview_lane_agents_restrict_to_read_only_tools() {
    let disallowed: &[&str] = &["Agent", "Edit", "Write", "NotebookEdit", "ExitPlanMode"];
    for agent in PIPELINE_AGENT_IDS.iter().chain(LANE_AGENT_IDS.iter()) {
        let path = format!("resources/agents/{agent}.yaml");
        let spec = load_agent(&path);
        for forbidden in disallowed {
            assert!(
                spec.disallowed_tools
                    .iter()
                    .any(|t| t.eq_ignore_ascii_case(forbidden)),
                "{path} must disallow {forbidden}"
            );
        }
        let allowed: Vec<&str> = spec.tools.iter().map(|s| s.as_str()).collect();
        assert!(
            allowed
                .iter()
                .all(|t| matches!(*t, "Read" | "Glob" | "Grep" | "Bash")),
            "{path} allowed_tools must be a subset of read-only tools, got {allowed:?}"
        );
    }
}

/// `reviewer-duplication` legitimately omits BLOCKER (duplication never
/// blocks a merge); the other four lanes must define all three.
#[test]
fn ultrareview_lane_agents_declare_severity_vocabulary() {
    for agent in LANE_AGENT_IDS {
        let path = format!("resources/agents/{agent}.yaml");
        let spec = load_agent(&path);
        assert!(
            spec.prompt.contains("SHOULD-FIX") && spec.prompt.contains("NIT"),
            "{path} must define SHOULD-FIX/NIT"
        );
        if *agent != "reviewer-duplication" {
            assert!(
                spec.prompt.contains("BLOCKER"),
                "{path} must define BLOCKER"
            );
        }
    }
}
