use puffer_resources::{PromptTemplate, ToolSpec};
use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .unwrap()
}

fn read_repo_file(relative_path: &str) -> String {
    fs::read_to_string(repo_root().join(relative_path)).unwrap()
}

fn claude_reference_available() -> bool {
    repo_root().join("references/claude-code").is_dir()
}

macro_rules! require_claude_reference {
    () => {
        if !claude_reference_available() {
            eprintln!("skipping Claude reference parity test; references/claude-code is absent");
            return;
        }
    };
}

fn load_prompt(relative_path: &str) -> PromptTemplate {
    serde_yaml::from_str(&read_repo_file(relative_path)).unwrap()
}

fn load_tool(relative_path: &str) -> ToolSpec {
    serde_yaml::from_str(&read_repo_file(relative_path)).unwrap()
}

fn render_prompt(relative_path: &str, variables: &[(&str, &str)]) -> String {
    let prompt = load_prompt(relative_path);
    prompt.render(
        &variables
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect(),
    )
}

fn extract_template_literal(contents: &str, marker: &str) -> String {
    let start = contents.find(marker).unwrap() + marker.len();
    let source = &contents[start..];
    let mut end = None;
    let mut index = 0usize;
    let mut escaped = false;
    let mut interpolation_depth = 0usize;

    while index < source.len() {
        let ch = source[index..].chars().next().unwrap();
        let width = ch.len_utf8();
        if escaped {
            escaped = false;
            index += width;
            continue;
        }
        if ch == '\\' {
            escaped = true;
            index += width;
            continue;
        }
        if interpolation_depth == 0 && ch == '`' {
            end = Some(start + index);
            break;
        }
        if source[index..].starts_with("${") {
            interpolation_depth += 1;
            index += 2;
            continue;
        }
        if interpolation_depth > 0 {
            match ch {
                '{' => interpolation_depth += 1,
                '}' => interpolation_depth = interpolation_depth.saturating_sub(1),
                _ => {}
            }
        }
        index += width;
    }

    contents[start..end.unwrap()].to_string()
}

fn extract_template_literal_after(contents: &str, anchor: &str, marker: &str) -> String {
    let start = contents.find(anchor).unwrap();
    extract_template_literal(&contents[start..], marker)
}

fn normalize_reference_template(raw: &str) -> String {
    let unescaped = raw.replace("\\`", "`");
    let trimmed = unescaped.strip_prefix('\n').unwrap_or(&unescaped);
    dedent(trimmed)
}

fn dedent(raw: &str) -> String {
    let indent = raw
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.chars().take_while(|ch| *ch == ' ').count())
        .min()
        .unwrap_or(0);
    raw.lines()
        .map(|line| line.strip_prefix(&" ".repeat(indent)).unwrap_or(line))
        .collect::<Vec<_>>()
        .join("\n")
}

fn strip_frontmatter(markdown: &str) -> String {
    let trimmed = markdown.trim_start();
    if !trimmed.starts_with("---\n") {
        return trimmed.to_string();
    }
    let remainder = &trimmed[4..];
    let end = remainder.find("\n---\n").unwrap();
    remainder[end + 5..].trim_start_matches('\n').to_string()
}

fn fenced(output: &str) -> String {
    format!("```\n{output}\n```")
}

#[test]
fn init_prompt_matches_claude_reference() {
    require_claude_reference!();
    let prompt = load_prompt("resources/prompts/init.yaml");
    let reference = read_repo_file("references/claude-code/src/commands/init.ts");
    let expected = normalize_reference_template(&extract_template_literal(
        &reference,
        "const NEW_INIT_PROMPT = `",
    ));

    assert_eq!(prompt.template.trim_end(), expected.trim_end());
}

#[test]
fn review_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt("resources/prompts/review.yaml", &[("ARGUMENTS", "123")]);
    let reference = read_repo_file("references/claude-code/src/commands/review.ts");
    let expected = normalize_reference_template(&extract_template_literal(
        &reference,
        "const LOCAL_REVIEW_PROMPT = (args: string) => `",
    ))
    .replace("${args}", "123");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn pr_comments_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/pr-comments.yaml",
        &[(
            "ADDITIONAL_USER_INPUT_BLOCK",
            "Additional user input: focus on unresolved threads",
        )],
    );
    let reference = read_repo_file("references/claude-code/src/commands/pr_comments/index.ts");
    let expected = normalize_reference_template(&extract_template_literal(&reference, "text: `"))
        .replace(
            "${args ? 'Additional user input: ' + args : ''}",
            "Additional user input: focus on unresolved threads",
        );

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn security_review_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let git_status = "On branch main\nnothing to commit, working tree clean";
    let files_modified = "src/lib.rs";
    let commits = "abc123 tighten prompt parity";
    let diff_content = "diff --git a/src/lib.rs b/src/lib.rs";
    let rendered = render_prompt(
        "resources/prompts/security-review.yaml",
        &[
            ("GIT_STATUS", git_status),
            ("FILES_MODIFIED", files_modified),
            ("COMMITS", commits),
            ("DIFF_CONTENT", diff_content),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/commands/security-review.ts");
    let expected = strip_frontmatter(&normalize_reference_template(&extract_template_literal(
        &reference,
        "const SECURITY_REVIEW_MARKDOWN = `",
    )))
    .replace("```\n!`git status`\n```", &fenced(git_status))
    .replace(
        "```\n!`git diff --name-only origin/HEAD...`\n```",
        &fenced(files_modified),
    )
    .replace(
        "```\n!`git log --no-decorate origin/HEAD...`\n```",
        &fenced(commits),
    )
    .replace(
        "```\n!`git diff origin/HEAD...`\n```",
        &fenced(diff_content),
    );

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn statusline_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/statusline.yaml",
        &[("STATUSLINE_PROMPT_JSON", "\"Mirror my starship prompt\"")],
    );
    let reference = read_repo_file("references/claude-code/src/commands/statusline.tsx");
    let expected = normalize_reference_template(&extract_template_literal(&reference, "text: `"))
        .replace("${AGENT_TOOL_NAME}", "Agent")
        .replace("${prompt}", "Mirror my starship prompt");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn commit_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let prompt = load_prompt("resources/prompts/commit.yaml");
    let rendered = prompt.render(&std::collections::BTreeMap::from([
        ("GIT_STATUS".to_string(), "STATUS".to_string()),
        ("GIT_DIFF".to_string(), "DIFF".to_string()),
        ("CURRENT_BRANCH".to_string(), "BRANCH".to_string()),
        ("RECENT_COMMITS".to_string(), "COMMITS".to_string()),
        ("COMMIT_ATTRIBUTION_BLOCK".to_string(), String::new()),
    ]));
    let reference = read_repo_file("references/claude-code/src/commands/commit.ts");
    let expected =
        normalize_reference_template(&extract_template_literal(&reference, "return `${prefix}"))
            .replace("!`git status`", "STATUS")
            .replace("!`git diff HEAD`", "DIFF")
            .replace("!`git branch --show-current`", "BRANCH")
            .replace("!`git log --oneline -10`", "COMMITS")
            .replace(
                r#"${commitAttribution ? `\n\n${commitAttribution}` : ''}"#,
                "",
            );

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn ask_user_question_tool_prompt_matches_claude_reference() {
    require_claude_reference!();
    let tool = load_tool("resources/tools/ask_user_question.yaml");
    let reference =
        read_repo_file("references/claude-code/src/tools/AskUserQuestionTool/prompt.ts");
    let prompt = normalize_reference_template(&extract_template_literal(
        &reference,
        "export const ASK_USER_QUESTION_TOOL_PROMPT = `",
    ))
    .replace("${EXIT_PLAN_MODE_TOOL_NAME}", "ExitPlanMode");
    let preview =
        normalize_reference_template(&extract_template_literal(&reference, "markdown: `"));
    let expected = format!("{prompt}\n{preview}").replace(
        "- Users will always be able to select \"Other\" to provide custom text input\n- Use multiSelect: true to allow multiple answers to be selected for a question",
        "- Use `type: \"choice\"` or omit `type` when the user should select from options\n- Use `type: \"input\"` when the user should type a value such as a phone number, login code, password, host, URL, or sender list\n- Choice questions let users select \"Other\" to provide custom text input\n- Use `searchable: true` for single-select catalog questions where users should filter a longer option list\n- Input questions collect the typed answer directly\n- Use multiSelect: true to allow multiple answers to be selected for a question",
    );

    assert_eq!(tool.description.trim_end(), expected.trim_end());
}

#[test]
fn enter_plan_mode_tool_prompt_matches_claude_reference() {
    require_claude_reference!();
    let tool = load_tool("resources/tools/enter_plan_mode.yaml");
    let reference = read_repo_file("references/claude-code/src/tools/EnterPlanModeTool/prompt.ts");
    let what_happens = normalize_reference_template(&extract_template_literal(
        &reference,
        "const WHAT_HAPPENS_SECTION = `",
    ))
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion");
    let expected = normalize_reference_template(&extract_template_literal(&reference, "return `"))
        .replace("${whatHappens}", &format!("{what_happens}\n"))
        .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion");

    assert_eq!(tool.description.trim_end(), expected.trim_end());
}

#[test]
fn exit_plan_mode_tool_prompt_matches_claude_reference() {
    require_claude_reference!();
    let tool = load_tool("resources/tools/exit_plan_mode.yaml");
    let reference = read_repo_file("references/claude-code/src/tools/ExitPlanModeTool/prompt.ts");
    let expected = normalize_reference_template(&extract_template_literal(
        &reference,
        "export const EXIT_PLAN_MODE_V2_TOOL_PROMPT = `",
    ))
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion");

    assert_eq!(tool.description.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_interview_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/plan-mode-interview.yaml",
        &[
            (
                "PLAN_FILE_INFO",
                "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool.",
            ),
            ("READ_ONLY_TOOL_NAMES", "Read, Glob, Grep"),
            (
                "EXPLORE_AGENT_HINT",
                " You can use the explore agent type to parallelize complex searches without filling your context, though for straightforward queries direct tools are simpler.",
            ),
            ("ASK_USER_QUESTION_TOOL_NAME", "AskUserQuestion"),
            ("EXIT_PLAN_MODE_TOOL_NAME", "ExitPlanMode"),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "function getPlanModeInterviewInstructions(",
        "const content = `",
    ))
    .replace("${planFileInfo}", "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool.")
    .replace("${getReadOnlyToolNames()}", "Read, Glob, Grep")
    .replace(
        r#"${areExplorePlanAgentsEnabled() ? ` You can use the ${EXPLORE_AGENT.agentType} agent type to parallelize complex searches without filling your context, though for straightforward queries direct tools are simpler.` : ''}"#,
        " You can use the explore agent type to parallelize complex searches without filling your context, though for straightforward queries direct tools are simpler.",
    )
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion")
    .replace("${ExitPlanModeV2Tool.name}", "ExitPlanMode");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_full_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let phase4_section = "### Phase 4: Final Plan\nGoal: Write your final plan to the plan file (the only file you can edit).\n- Begin with a **Context** section: explain why this change is being made — the problem or need it addresses, what prompted it, and the intended outcome\n- Include only your recommended approach, not all alternatives\n- Ensure that the plan file is concise enough to scan quickly, but detailed enough to execute effectively\n- Include the paths of critical files to be modified\n- Reference existing functions and utilities you found that should be reused, with their file paths\n- Include a verification section describing how to test the changes end-to-end (run the code, use MCP tools, run tests)";
    let rendered = render_prompt(
        "resources/prompts/plan-mode-full.yaml",
        &[
            (
                "PLAN_FILE_INFO",
                "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool.",
            ),
            ("EXPLORE_AGENT_TYPE", "explore"),
            ("EXPLORE_AGENT_COUNT", "1"),
            ("PLAN_AGENT_TYPE", "plan"),
            ("PLAN_AGENT_COUNT", "1"),
            ("MULTI_AGENT_GUIDANCE", ""),
            ("PHASE4_SECTION", phase4_section),
            ("ASK_USER_QUESTION_TOOL_NAME", "AskUserQuestion"),
            ("EXIT_PLAN_MODE_TOOL_NAME", "ExitPlanMode"),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "function getPlanModeV2Instructions(",
        "const content = `",
    ))
    .replace("${planFileInfo}", "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool.")
    .replace("${EXPLORE_AGENT.agentType}", "explore")
    .replace("${exploreAgentCount}", "1")
    .replace("${PLAN_AGENT.agentType}", "plan")
    .replace("${agentCount}", "1")
    .replace(
        "${\n  agentCount > 1\n    ? `- **Multiple agents**: Use up to 1 agents for complex tasks that benefit from different perspectives\n\nExamples of when to use multiple agents:\n- The task touches multiple parts of the codebase\n- It's a large refactor or architectural change\n- There are many edge cases to consider\n- You'd benefit from exploring different approaches\n\nExample perspectives by task type:\n- New feature: simplicity vs performance vs maintainability\n- Bug fix: root cause vs workaround vs prevention\n- Refactoring: minimal change vs clean architecture\n`\n    : ''\n}",
        "",
    )
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion")
    .replace("${ExitPlanModeV2Tool.name}", "ExitPlanMode")
    .replace("${getPlanPhase4Section()}", phase4_section);

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_sparse_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/plan-mode-sparse.yaml",
        &[
            ("PLAN_FILE_PATH", "/tmp/plan.md"),
            (
                "WORKFLOW_DESCRIPTION",
                "Follow iterative workflow: explore codebase, interview user, write to plan incrementally.",
            ),
            ("ASK_USER_QUESTION_TOOL_NAME", "AskUserQuestion"),
            ("EXIT_PLAN_MODE_TOOL_NAME", "ExitPlanMode"),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "function getPlanModeV2SparseInstructions(",
        "const content = `",
    ))
    .replace("${attachment.planFilePath}", "/tmp/plan.md")
    .replace(
        "${workflowDescription}",
        "Follow iterative workflow: explore codebase, interview user, write to plan incrementally.",
    )
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion")
    .replace("${ExitPlanModeV2Tool.name}", "ExitPlanMode");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_subagent_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/plan-mode-subagent.yaml",
        &[
            (
                "PLAN_FILE_INFO",
                "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool if you need to.",
            ),
            ("ASK_USER_QUESTION_TOOL_NAME", "AskUserQuestion"),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "function getPlanModeV2SubAgentInstructions(",
        "const content = `",
    ))
    .replace("${planFileInfo}", "A plan file already exists at /tmp/plan.md. You can read it and make incremental edits using the Edit tool if you need to.")
    .replace("${ASK_USER_QUESTION_TOOL_NAME}", "AskUserQuestion");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_reentry_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/plan-mode-reentry.yaml",
        &[
            ("PLAN_FILE_PATH", "/tmp/plan.md"),
            ("EXIT_PLAN_MODE_TOOL_NAME", "ExitPlanMode"),
        ],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "case 'plan_mode_reentry': {",
        "const content = `",
    ))
    .replace("${attachment.planFilePath}", "/tmp/plan.md")
    .replace("${ExitPlanModeV2Tool.name}", "ExitPlanMode");

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn plan_mode_exited_prompt_matches_claude_reference_when_rendered() {
    require_claude_reference!();
    let rendered = render_prompt(
        "resources/prompts/plan-mode-exited.yaml",
        &[(
            "PLAN_REFERENCE",
            " The plan file is located at /tmp/plan.md if you need to reference it.",
        )],
    );
    let reference = read_repo_file("references/claude-code/src/utils/messages.ts");
    let expected = normalize_reference_template(&extract_template_literal_after(
        &reference,
        "case 'plan_mode_exit': {",
        "const content = `",
    ))
    .replace(
        "${planReference}",
        " The plan file is located at /tmp/plan.md if you need to reference it.",
    );

    assert_eq!(rendered.trim_end(), expected.trim_end());
}

#[test]
fn todo_write_tool_prompt_matches_claude_reference() {
    require_claude_reference!();
    let tool = load_tool("resources/tools/todo_write.yaml");
    let reference = read_repo_file("references/claude-code/src/tools/TodoWriteTool/prompt.ts");
    let expected = normalize_reference_template(&extract_template_literal(
        &reference,
        "export const PROMPT = `",
    ))
    .replace("${FILE_EDIT_TOOL_NAME}", "Edit");

    assert_eq!(tool.description.trim_end(), expected.trim_end());
}
