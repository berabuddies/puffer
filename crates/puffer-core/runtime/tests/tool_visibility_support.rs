fn bundled_resources() -> LoadedResources {
    let root = workspace_root();
    let temp = tempfile::tempdir().unwrap();
    let paths = ConfigPaths {
        workspace_root: temp.path().join("workspace"),
        workspace_config_dir: temp.path().join("workspace/.puffer"),
        user_config_dir: temp.path().join("user"),
        builtin_resources_dir: root.join("resources"),
    };
    load_resources(&paths, &crate::runner_adapter::LocalToolRunner::new()).unwrap()
}

fn workspace_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("workspace root")
        .to_path_buf()
}

fn read_repo_file(relative_path: &str) -> String {
    fs::read_to_string(workspace_root().join(relative_path)).unwrap()
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

fn normalize_reference_template(raw: &str) -> String {
    let unescaped = raw.replace("\\`", "`");
    let trimmed = unescaped.strip_prefix('\n').unwrap_or(&unescaped);
    dedent(trimmed)
}

fn decode_js_template_escapes(raw: &str) -> String {
    let mut output = String::new();
    let mut chars = raw.chars();
    while let Some(ch) = chars.next() {
        if ch != '\\' {
            output.push(ch);
            continue;
        }
        match chars.next() {
            Some('\\') => output.push('\\'),
            Some('n') => output.push('\n'),
            Some('r') => output.push('\r'),
            Some('t') => output.push('\t'),
            Some('"') => output.push('"'),
            Some('`') => output.push('`'),
            Some(other) => {
                output.push('\\');
                output.push(other);
            }
            None => output.push('\\'),
        }
    }
    output
}

fn trim_line_trailing_whitespace(raw: &str) -> String {
    raw.lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
}

fn normalize_agent_prompt_text(raw: &str) -> String {
    normalize_inline_whitespace(
        &trim_line_trailing_whitespace(&decode_js_template_escapes(raw)).replace(
            "file creation or modification",
            "file creation/modification",
        ),
    )
    .replace(['—', '–'], "-")
    .replace(['“', '”'], "\"")
    .replace(['’', '‘'], "'")
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

fn current_month_year() -> String {
    let now = OffsetDateTime::now_local().unwrap_or_else(|_| OffsetDateTime::now_utc());
    let format =
        format_description::parse("[month repr:long] [year]").expect("valid month/year format");
    now.format(&format).unwrap()
}

fn assert_tool_description_matches_expected(
    registry: &ToolRegistry,
    tool_id: &str,
    expected: &str,
) {
    let expected = normalize_tool_description(tool_id, expected);
    let description = registry
        .definition(tool_id)
        .expect("tool definition")
        .description
        .clone();
    assert_eq!(normalize_tool_description(tool_id, &description), expected);

    let anthropic = anthropic_tool_definitions(registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|item| item["name"] == json!(tool_id))
        .expect("anthropic tool definition");
    assert_eq!(
        normalize_tool_description(
            tool_id,
            anthropic_definition["description"]
                .as_str()
                .expect("anthropic description"),
        ),
        expected,
        "anthropic description for {tool_id}"
    );

    let openai = openai_tool_definitions(registry, None, false).unwrap();
    let openai_definition = openai
        .iter()
        .find(|item| item.name == tool_id)
        .expect("openai tool definition");
    assert_eq!(
        normalize_tool_description(tool_id, &openai_definition.description),
        normalize_tool_description(
            tool_id,
            &expected_openai_tool_description(tool_id, tool_id, &expected)
        ),
        "openai description for {tool_id}"
    );
}

fn expected_openai_tool_description(tool_id: &str, name: &str, description: &str) -> String {
    match tool_id {
        "Agent" => "Delegate an independent subtask to a subagent. Use for large or parallelizable work, not simple file reads or searches.".to_string(),
        "AskUserQuestion" => {
            "Ask the user a short clarification or decision question when blocked by ambiguity."
                .to_string()
        }
        "TodoWrite" => {
            "Update the task list with pending, in_progress, and completed items. Keep at most one item in progress."
                .to_string()
        }
        "Read" => "Read a file. Prefer reading the whole file unless it is large; use offset or limit for partial reads.".to_string(),
        "Glob" => "Find files by path pattern. Prefer this over shelling out to find or ls for discovery.".to_string(),
        "Grep" => "Search file contents with ripgrep-style patterns. Prefer this over running grep or rg in Bash.".to_string(),
        "Edit" => "Make an exact text edit in an existing file. Read the file first when needed.".to_string(),
        "Write" => "Write a file, creating parent directories if needed.".to_string(),
        "Bash" => "Run a shell command when no dedicated tool is a better fit.".to_string(),
        "TaskOutput" => "Read the saved output of a background task by id.".to_string(),
        "WebFetch" => "Fetch and summarize content from a specific URL.".to_string(),
        "WebSearch" => format!(
            "Search the web for current or external information. The current month is {} and you must use this year when searching for recent information.",
            current_month_year()
        ),
        _ => compact_openai_tool_description(name, description),
    }
}

fn compact_openai_tool_description(name: &str, description: &str) -> String {
    let trimmed = description.trim();
    if trimmed.is_empty() {
        return name.to_string();
    }
    let first_paragraph = trimmed.split("\n\n").next().unwrap_or(trimmed).trim();
    if first_paragraph.len() <= 220 {
        first_paragraph.to_string()
    } else {
        let mut shortened = first_paragraph.chars().take(217).collect::<String>();
        shortened.push_str("...");
        shortened
    }
}

fn normalize_tool_description(tool_id: &str, raw: &str) -> String {
    let trimmed = trim_line_trailing_whitespace(raw);
    if tool_id == "PowerShell" {
        return trimmed
            .replace(
                "\n- You can use the `run_in_background` parameter",
                "\n  - You can use the `run_in_background` parameter",
            )
            .replace(
                "\n\n  - Avoid using PowerShell to run commands that have dedicated tools, unless explicitly instructed:",
                "\n  - Avoid using PowerShell to run commands that have dedicated tools, unless explicitly instructed:",
            )
            .replace(
                "\n\n  - For git commands:",
                "\n  - For git commands:",
            );
    }
    trimmed
}

fn reference_sleep_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/SleepTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(
        &reference,
        "export const SLEEP_TOOL_PROMPT = `",
    ))
    .replace("${TICK_TAG}", "tick")
}

fn reference_lsp_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/LSPTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(
        &reference,
        "export const DESCRIPTION = `",
    ))
}

fn reference_powershell_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/PowerShellTool/prompt.ts");
    let prompt = normalize_reference_template(&extract_template_literal(
        reference
            .split("export async function getPrompt(): Promise<string> {")
            .nth(1)
            .expect("PowerShell prompt section"),
        "  return `",
    ));
    let background = normalize_reference_template(&extract_template_literal(
        reference
            .split("function getBackgroundUsageNote(): string | null {")
            .nth(1)
            .expect("PowerShell background section"),
        "  return `",
    ));
    let sleep = normalize_reference_template(&extract_template_literal(
        reference
            .split("function getSleepGuidance(): string | null {")
            .nth(1)
            .expect("PowerShell sleep section"),
        "  return `",
    ));
    let edition = normalize_reference_template(&extract_template_literal(
        reference
            .split("// Detection not yet resolved (first prompt build before any tool call) or")
            .nth(1)
            .expect("PowerShell unknown edition section"),
        "  return `",
    ));
    decode_js_escapes(
        &prompt
            .replace("${getEditionSection(edition)}", &edition)
            .replace("${getMaxTimeoutMs()}", "600000")
            .replace("${getMaxTimeoutMs() / 60000}", "10")
            .replace("${getDefaultTimeoutMs()}", "120000")
            .replace("${getDefaultTimeoutMs() / 60000}", "2")
            .replace("${getMaxOutputLength()}", "30000")
            .replace(
                "${backgroundNote ? backgroundNote + '\\n' : ''}\\",
                &(indent_block(&background, 2) + "\n"),
            )
            .replace(
                "${sleepGuidance ? sleepGuidance + '\\n' : ''}\\",
                &(indent_block(&sleep, 2) + "\n\n"),
            )
            .replace("${GLOB_TOOL_NAME}", "Glob")
            .replace("${GREP_TOOL_NAME}", "Grep")
            .replace("${FILE_READ_TOOL_NAME}", "Read")
            .replace("${FILE_EDIT_TOOL_NAME}", "Edit")
            .replace("${FILE_WRITE_TOOL_NAME}", "Write")
            .replace("${POWERSHELL_TOOL_NAME}", "PowerShell")
            .replace("\n\n\n  - For git commands:", "\n\n  - For git commands:"),
    )
}

fn reference_task_create_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/TaskCreateTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
        .replace("${teammateContext}", " and potentially assigned to teammates")
        .replace(
            "${teammateTips}",
            "- Include enough detail in the description for another agent to understand and complete the task\n- New tasks are created with status 'pending' and no owner - use TaskUpdate with the `owner` parameter to assign them\n",
        )
}

fn reference_task_update_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/TaskUpdateTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(
        &reference,
        "export const PROMPT = `",
    ))
}

fn reference_team_create_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/TeamCreateTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
}

fn reference_team_delete_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/TeamDeleteTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
}

fn reference_enter_worktree_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/EnterWorktreeTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
}

fn reference_exit_worktree_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/ExitWorktreeTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
}

fn reference_list_mcp_resources_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/ListMcpResourcesTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(
        &reference,
        "export const PROMPT = `",
    ))
}

fn reference_read_mcp_resource_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/ReadMcpResourceTool/prompt.ts");
    normalize_reference_template(&extract_template_literal(
        &reference,
        "export const PROMPT = `",
    ))
}

fn reference_tool_search_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/ToolSearchTool/prompt.ts");
    let head = normalize_reference_template(&extract_template_literal(
        &reference,
        "const PROMPT_HEAD = `",
    ));
    let tail = normalize_reference_template(&extract_template_literal(
        &reference,
        "const PROMPT_TAIL = `",
    ));
    format!("{head}Deferred tools appear by name in <system-reminder> messages.{tail}")
}

fn reference_web_fetch_prompt() -> String {
    let prompt_reference =
        read_repo_file("references/claude-code/src/tools/WebFetchTool/prompt.ts");
    let description = normalize_reference_template(&extract_template_literal(
        &prompt_reference,
        "export const DESCRIPTION = `",
    ));
    let tool_reference =
        read_repo_file("references/claude-code/src/tools/WebFetchTool/WebFetchTool.ts");
    let prompt_section = &tool_reference[tool_reference
        .find("  async prompt(_options) {")
        .expect("WebFetch prompt function")..];
    normalize_reference_template(&extract_template_literal(prompt_section, "    return `"))
        .replace("${DESCRIPTION}", &description)
}

fn reference_file_read_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/FileReadTool/prompt.ts");
    let template =
        normalize_reference_template(&extract_template_literal(&reference, "  return `"));
    let line_format =
        extract_quoted_literal(&reference, "export const LINE_FORMAT_INSTRUCTION =\n  ");
    let offset_instruction =
        extract_quoted_literal(&reference, "export const OFFSET_INSTRUCTION_DEFAULT =\n  ");
    let pdf_instruction = extract_quoted_literal(&reference, "      ? ");
    render_reference_template_literal(&template, |expr| match expr.trim() {
        "MAX_LINES_TO_READ" => "2000".to_string(),
        "maxSizeInstruction" => String::new(),
        "offsetInstruction" => offset_instruction.clone(),
        "lineFormat" => line_format.clone(),
        "BASH_TOOL_NAME" => "Bash".to_string(),
        expr if expr.contains("isPDFSupported()") => pdf_instruction.clone(),
        other => panic!("unexpected Read template expression: {other}"),
    })
}

fn reference_file_edit_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/FileEditTool/prompt.ts");
    let pre_read_section = reference
        .split("function getPreReadInstruction(): string {")
        .nth(1)
        .expect("Read pre-instruction section");
    let pre_read_template =
        normalize_reference_template(&extract_template_literal(pre_read_section, "  return `"));
    let pre_read =
        render_reference_template_literal(&pre_read_template, |expr| match expr.trim() {
            "FILE_READ_TOOL_NAME" => "Read".to_string(),
            other => panic!("unexpected Edit pre-read expression: {other}"),
        });

    let description_section = reference
        .split("function getDefaultEditDescription(): string {")
        .nth(1)
        .expect("Edit description section");
    let description_template =
        normalize_reference_template(&extract_template_literal(description_section, "  return `"));
    render_reference_template_literal(&description_template, |expr| match expr.trim() {
        "getPreReadInstruction()" => pre_read.clone(),
        "prefixFormat" => "line number + tab".to_string(),
        "minimalUniquenessHint" => String::new(),
        other => panic!("unexpected Edit template expression: {other}"),
    })
}

fn reference_file_write_prompt() -> String {
    let reference = read_repo_file("references/claude-code/src/tools/FileWriteTool/prompt.ts");
    let pre_read_section = reference
        .split("function getPreReadInstruction(): string {")
        .nth(1)
        .expect("Write pre-instruction section");
    let pre_read_template =
        normalize_reference_template(&extract_template_literal(pre_read_section, "  return `"));
    let pre_read =
        render_reference_template_literal(&pre_read_template, |expr| match expr.trim() {
            "FILE_READ_TOOL_NAME" => "Read".to_string(),
            other => panic!("unexpected Write pre-read expression: {other}"),
        });

    let description_section = reference
        .split("export function getWriteToolDescription(): string {")
        .nth(1)
        .expect("Write description section");
    let description_template =
        normalize_reference_template(&extract_template_literal(description_section, "  return `"));
    render_reference_template_literal(&description_template, |expr| match expr.trim() {
        "getPreReadInstruction()" => pre_read.clone(),
        other => panic!("unexpected Write template expression: {other}"),
    })
}

fn reference_file_read_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "The absolute path to the file to read"
            },
            "offset": {
                "type": "integer",
                "description": "The line number to start reading from. Only provide if the file is too large to read at once",
                "minimum": 0
            },
            "limit": {
                "type": "integer",
                "description": "The number of lines to read. Only provide if the file is too large to read at once.",
                "minimum": 1
            },
            "pages": {
                "type": "string",
                "description": "Page range for PDF files (e.g., \"1-5\", \"3\", \"10-20\"). Only applicable to PDF files. Maximum 20 pages per request."
            }
        },
        "required": ["file_path"],
        "additionalProperties": false
    })
}

fn reference_file_edit_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "The absolute path to the file to modify"
            },
            "old_string": {
                "type": "string",
                "description": "The text to replace"
            },
            "new_string": {
                "type": "string",
                "description": "The text to replace it with (must be different from old_string)"
            },
            "replace_all": {
                "type": "boolean",
                "description": "Replace all occurrences of old_string (default false)"
            }
        },
        "required": ["file_path", "old_string", "new_string"],
        "additionalProperties": false
    })
}

fn reference_file_write_schema() -> serde_json::Value {
    json!({
        "type": "object",
        "properties": {
            "file_path": {
                "type": "string",
                "description": "The absolute path to the file to write (must be absolute, not relative)"
            },
            "content": {
                "type": "string",
                "description": "The content to write to the file"
            }
        },
        "required": ["file_path", "content"],
        "additionalProperties": false
    })
}

fn extract_quoted_literal(contents: &str, marker: &str) -> String {
    let start = contents.find(marker).unwrap() + marker.len();
    let source = &contents[start..];
    let quote = source.chars().next().unwrap();
    assert!(quote == '\'' || quote == '"');
    let mut end = None;
    let mut index = quote.len_utf8();
    let mut escaped = false;

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
        if ch == quote {
            end = Some(index);
            break;
        }
        index += width;
    }

    decode_js_escapes(&source[quote.len_utf8()..end.unwrap()])
}

fn render_reference_template_literal(
    template: &str,
    mut replacer: impl FnMut(&str) -> String,
) -> String {
    let mut rendered = String::new();
    let mut index = 0usize;

    while index < template.len() {
        if template[index..].starts_with("${") {
            let expression_start = index + 2;
            let mut cursor = expression_start;
            let mut depth = 1usize;
            while cursor < template.len() {
                let ch = template[cursor..].chars().next().unwrap();
                let width = ch.len_utf8();
                if template[cursor..].starts_with("${") {
                    depth += 1;
                    cursor += 2;
                    continue;
                }
                match ch {
                    '{' => depth += 1,
                    '}' => {
                        depth = depth.saturating_sub(1);
                        if depth == 0 {
                            let expression = &template[expression_start..cursor];
                            rendered.push_str(&replacer(expression));
                            cursor += width;
                            index = cursor;
                            break;
                        }
                    }
                    _ => {}
                }
                cursor += width;
            }
            continue;
        }

        let ch = template[index..].chars().next().unwrap();
        rendered.push(ch);
        index += ch.len_utf8();
    }

    decode_js_escapes(&rendered)
}

fn decode_js_escapes(raw: &str) -> String {
    let mut decoded = String::new();
    let mut chars = raw.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch != '\\' {
            decoded.push(ch);
            continue;
        }

        match chars.next() {
            Some('n') => decoded.push('\n'),
            Some('r') => decoded.push('\r'),
            Some('t') => decoded.push('\t'),
            Some('\n') => {}
            Some('\r') => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
            }
            Some('\'') => decoded.push('\''),
            Some('"') => decoded.push('"'),
            Some('`') => decoded.push('`'),
            Some('\\') => decoded.push('\\'),
            Some('u') => {
                let mut code = String::new();
                for _ in 0..4 {
                    code.push(chars.next().expect("unicode escape"));
                }
                let scalar = u32::from_str_radix(&code, 16).expect("valid unicode escape");
                decoded.push(char::from_u32(scalar).expect("unicode scalar"));
            }
            Some(other) => {
                decoded.push('\\');
                decoded.push(other);
            }
            None => decoded.push('\\'),
        }
    }

    decoded
}

fn normalize_inline_whitespace(raw: &str) -> String {
    raw.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn normalize_prompt_lines(raw: &str) -> String {
    let mut normalized = Vec::new();
    let mut previous_blank = false;

    for line in raw.lines().map(str::trim_end) {
        if line.is_empty() {
            if !previous_blank {
                normalized.push(String::new());
            }
            previous_blank = true;
            continue;
        }

        normalized.push(line.to_string());
        previous_blank = false;
    }

    normalized.join("\n")
}

fn indent_block(block: &str, spaces: usize) -> String {
    let prefix = " ".repeat(spaces);
    block
        .lines()
        .map(|line| {
            if line.is_empty() {
                String::new()
            } else {
                format!("{prefix}{line}")
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn reference_explore_agent_description() -> String {
    "Fast agent specialized for exploring codebases. Use this when you need to quickly find files by patterns (eg. \"src/components/**/*.tsx\"), search code for keywords (eg. \"API endpoints\"), or answer questions about the codebase (eg. \"how do API endpoints work?\"). When calling this agent, specify the desired thoroughness level: \"quick\" for basic searches, \"medium\" for moderate exploration, or \"very thorough\" for comprehensive analysis across multiple locations and naming conventions.".to_string()
}

fn reference_explore_agent_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/AgentTool/built-in/exploreAgent.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
        .replace("${BASH_TOOL_NAME}", "Bash")
        .replace("${FILE_READ_TOOL_NAME}", "Read")
        .replace(
            "${globGuidance}",
            "- Use `Glob` for broad file pattern matching",
        )
        .replace(
            "${grepGuidance}",
            "- Use `Grep` for searching file contents with regex",
        )
        .replace("${embedded ? ', grep' : ''}", "")
}

fn reference_general_purpose_agent_description() -> String {
    "General-purpose agent for researching complex questions, searching for code, and executing multi-step tasks. When you are searching for a keyword or file and are not confident that you will find the right match in the first few tries use this agent to perform the search for you.".to_string()
}

fn reference_general_purpose_agent_prompt() -> String {
    let reference = read_repo_file(
        "references/claude-code/src/tools/AgentTool/built-in/generalPurposeAgent.ts",
    );
    let prefix = normalize_reference_template(&extract_template_literal(
        &reference,
        "const SHARED_PREFIX = `",
    ));
    let guidelines = normalize_reference_template(&extract_template_literal(
        &reference,
        "const SHARED_GUIDELINES = `",
    ));
    format!(
        "{prefix} When you complete the task, respond with a concise report covering what was done and any key findings — the caller will relay this to the user, so it only needs the essentials.\n\n{guidelines}"
    )
}

fn reference_plan_agent_description() -> String {
    "Software architect agent for designing implementation plans. Use this when you need to plan the implementation strategy for a task. Returns step-by-step plans, identifies critical files, and considers architectural trade-offs.".to_string()
}

fn reference_plan_agent_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/AgentTool/built-in/planAgent.ts");
    normalize_reference_template(&extract_template_literal(&reference, "  return `"))
        .replace("${searchToolsHint}", "Glob, Grep, and Read")
        .replace("${BASH_TOOL_NAME}", "Bash")
        .replace("${hasEmbeddedSearchTools() ? ', grep' : ''}", "")
}

fn reference_statusline_setup_agent_description() -> String {
    "Use this agent to configure the user's Claude Code status line setting.".to_string()
}

fn reference_statusline_setup_agent_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/AgentTool/built-in/statuslineSetup.ts");
    trim_line_trailing_whitespace(&decode_js_template_escapes(&normalize_reference_template(
        &extract_template_literal(&reference, "const STATUSLINE_SYSTEM_PROMPT = `"),
    )))
}

fn reference_verification_agent_description() -> String {
    "Use this agent to verify that implementation work is correct before reporting completion. Invoke after non-trivial tasks (3+ file edits, backend/API changes, infrastructure changes). Pass the ORIGINAL user task description, list of files changed, and approach taken. The agent runs builds, tests, linters, and checks to produce a PASS/FAIL/PARTIAL verdict with evidence.".to_string()
}

fn reference_verification_agent_prompt() -> String {
    let reference =
        read_repo_file("references/claude-code/src/tools/AgentTool/built-in/verificationAgent.ts");
    trim_line_trailing_whitespace(&decode_js_template_escapes(
        &normalize_reference_template(&extract_template_literal(
            &reference,
            "const VERIFICATION_SYSTEM_PROMPT = `",
        ))
        .replace("${BASH_TOOL_NAME}", "Bash")
        .replace("${WEB_FETCH_TOOL_NAME}", "WebFetch"),
    ))
}
