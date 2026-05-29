use super::*;
use puffer_resources::load_resources;
use serde_json::json;
use std::fs;
use std::path::{Path, PathBuf};
use time::{format_description, OffsetDateTime};

include!("tool_visibility_support.rs");

const NOTEBOOK_EDIT_DESCRIPTION: &str =
    "Completely replaces the contents of a specific cell in a Jupyter notebook\n(.ipynb file) with new source.\n\nJupyter notebooks combine code, text, and visualizations for data analysis\nand scientific computing.\n\nUsage:\n- `notebook_path` must be an absolute path.\n- Read the notebook with `Read` before editing it. This tool fails if the\n  notebook has not been fully read or if it changed after it was read.\n- `cell_id` identifies the target cell. Existing cell ids and `cell-N` or\n  numeric index fallbacks are accepted.\n- Use `edit_mode: \"insert\"` to add a new cell after `cell_id`, or at the\n  beginning if `cell_id` is omitted.\n- Use `edit_mode: \"delete\"` to remove the target cell.\n- `cell_type` is required when inserting and may be `code` or `markdown`.";
const CONFIG_TOOL_SNIPPETS: &[&str] = &[
    "Get or set Claude Code configuration settings.",
    "### Global Settings",
    "copy_full_response",
    "### Project Settings",
    "openai_headers",
    "### Session Settings",
    "statuslineEnabled",
];
const AGENT_TOOL_SNIPPETS: &[&str] = &[
    "Launch a new agent to handle complex, multi-step tasks autonomously.",
    "Available agent types and the tools they have access to:",
    "- general-purpose:",
    "- Explore:",
    "Launch multiple agents concurrently whenever possible",
    "To continue a previously spawned agent, use SendMessage",
    "`isolation: \"worktree\"`",
    "## Writing the prompt",
    "Never delegate understanding.",
];
const ASK_USER_QUESTION_DESCRIPTION: &str = "Use this tool when you need to ask the user questions during execution. This allows you to:\n1. Gather user preferences or requirements\n2. Clarify ambiguous instructions\n3. Get decisions on implementation choices as you work\n4. Offer choices to the user about what direction to take.\n\nUsage notes:\n- Users will always be able to select \"Other\" to provide custom text input\n- Use multiSelect: true to allow multiple answers to be selected for a question\n- If you recommend a specific option, make that the first option in the list and add \"(Recommended)\" at the end of the label\n\nPlan mode note: In plan mode, use this tool to clarify requirements or choose between approaches BEFORE finalizing your plan. Do NOT use this tool to ask \"Is my plan ready?\" or \"Should I proceed?\" - use ExitPlanMode for plan approval. IMPORTANT: Do not reference \"the plan\" in your questions (e.g., \"Do you have feedback about the plan?\", \"Does the plan look good?\") because the user cannot see the plan in the UI until you call ExitPlanMode. If you need plan approval, use ExitPlanMode instead.\nPreview feature:\nUse the optional `preview` field on options when presenting concrete artifacts that users need to visually compare:\n- ASCII mockups of UI layouts or components\n- Code snippets showing different implementations\n- Diagram variations\n- Configuration examples\n\nPreview content is rendered as markdown in a monospace box. Multi-line text with newlines is supported. When any option has a preview, the UI switches to a side-by-side layout with a vertical option list on the left and preview on the right. Do not use previews for simple preference questions where labels and descriptions suffice. Note: previews are only supported for single-select questions (not multiSelect).";
const ENTER_PLAN_MODE_DESCRIPTION: &str = "Use this tool proactively when you're about to start a non-trivial implementation task. Getting user sign-off on your approach before writing code prevents wasted effort and ensures alignment. This tool transitions you into plan mode where you can explore the codebase and design an implementation approach for user approval.\n\n## When to Use This Tool\n\n**Prefer using EnterPlanMode** for implementation tasks unless they're simple. Use it when ANY of these conditions apply:\n\n1. **New Feature Implementation**: Adding meaningful new functionality\n   - Example: \"Add a logout button\" - where should it go? What should happen on click?\n   - Example: \"Add form validation\" - what rules? What error messages?\n\n2. **Multiple Valid Approaches**: The task can be solved in several different ways\n   - Example: \"Add caching to the API\" - could use Redis, in-memory, file-based, etc.\n   - Example: \"Improve performance\" - many optimization strategies possible\n\n3. **Code Modifications**: Changes that affect existing behavior or structure\n   - Example: \"Update the login flow\" - what exactly should change?\n   - Example: \"Refactor this component\" - what's the target architecture?\n\n4. **Architectural Decisions**: The task requires choosing between patterns or technologies\n   - Example: \"Add real-time updates\" - WebSockets vs SSE vs polling\n   - Example: \"Implement state management\" - Redux vs Context vs custom solution\n\n5. **Multi-File Changes**: The task will likely touch more than 2-3 files\n   - Example: \"Refactor the authentication system\"\n   - Example: \"Add a new API endpoint with tests\"\n\n6. **Unclear Requirements**: You need to explore before understanding the full scope\n   - Example: \"Make the app faster\" - need to profile and identify bottlenecks\n   - Example: \"Fix the bug in checkout\" - need to investigate root cause\n\n7. **User Preferences Matter**: The implementation could reasonably go multiple ways\n   - If you would use AskUserQuestion to clarify the approach, use EnterPlanMode instead\n   - Plan mode lets you explore first, then present options with context\n\n## When NOT to Use This Tool\n\nOnly skip EnterPlanMode for simple tasks:\n- Single-line or few-line fixes (typos, obvious bugs, small tweaks)\n- Adding a single function with clear requirements\n- Tasks where the user has given very specific, detailed instructions\n- Pure research/exploration tasks (use the Agent tool with explore agent instead)\n\n## What Happens in Plan Mode\n\nIn plan mode, you'll:\n1. Thoroughly explore the codebase using Glob, Grep, and Read tools\n2. Understand existing patterns and architecture\n3. Design an implementation approach\n4. Present your plan to the user for approval\n5. Use AskUserQuestion if you need to clarify approaches\n6. Exit plan mode with ExitPlanMode when ready to implement\n\n## Examples\n\n### GOOD - Use EnterPlanMode:\nUser: \"Add user authentication to the app\"\n- Requires architectural decisions (session vs JWT, where to store tokens, middleware structure)\n\nUser: \"Optimize the database queries\"\n- Multiple approaches possible, need to profile first, significant impact\n\nUser: \"Implement dark mode\"\n- Architectural decision on theme system, affects many components\n\nUser: \"Add a delete button to the user profile\"\n- Seems simple but involves: where to place it, confirmation dialog, API call, error handling, state updates\n\nUser: \"Update the error handling in the API\"\n- Affects multiple files, user should approve the approach\n\n### BAD - Don't use EnterPlanMode:\nUser: \"Fix the typo in the README\"\n- Straightforward, no planning needed\n\nUser: \"Add a console.log to debug this function\"\n- Simple, obvious implementation\n\nUser: \"What files handle routing?\"\n- Research task, not implementation planning\n\n## Important Notes\n\n- This tool REQUIRES user approval - they must consent to entering plan mode\n- If unsure whether to use it, err on the side of planning - it's better to get alignment upfront than to redo work\n- Users appreciate being consulted before significant changes are made to their codebase";
const EXIT_PLAN_MODE_DESCRIPTION: &str = "Use this tool when you are in plan mode and have finished writing your plan to the plan file and are ready for user approval.\n\n## How This Tool Works\n- You should have already written your plan to the plan file specified in the plan mode system message\n- This tool does NOT take the plan content as a parameter - it will read the plan from the file you wrote\n- This tool simply signals that you're done planning and ready for the user to review and approve\n- The user will see the contents of your plan file when they review it\n\n## When to Use This Tool\nIMPORTANT: Only use this tool when the task requires planning the implementation steps of a task that requires writing code. For research tasks where you're gathering information, searching files, reading files or in general trying to understand the codebase - do NOT use this tool.\n\n## Before Using This Tool\nEnsure your plan is complete and unambiguous:\n- If you have unresolved questions about requirements or approach, use AskUserQuestion first (in earlier phases)\n- Once your plan is finalized, use THIS tool to request approval\n\n**Important:** Do NOT use AskUserQuestion to ask \"Is this plan okay?\" or \"Should I proceed?\" - that's exactly what THIS tool does. ExitPlanMode inherently requests user approval of your plan.\n\n## Examples\n\n1. Initial task: \"Search for and understand the implementation of vim mode in the codebase\" - Do not use the exit plan mode tool because you are not planning the implementation steps of a task.\n2. Initial task: \"Help me implement yank mode for vim\" - Use the exit plan mode tool after you have finished planning the implementation steps of the task.\n3. Initial task: \"Add a new feature to handle user authentication\" - If unsure about auth method (OAuth, JWT, etc.), use AskUserQuestion first, then use exit plan mode tool after clarifying the approach.";
const TODO_WRITE_DESCRIPTION: &str = "Use this tool to create and manage a structured task list for your current coding session. This helps you track progress, organize complex tasks, and demonstrate thoroughness to the user.\nIt also helps the user understand the progress of the task and overall progress of their requests.\n\n## When to Use This Tool\nUse this tool proactively in these scenarios:\n\n1. Complex multi-step tasks - When a task requires 3 or more distinct steps or actions\n2. Non-trivial and complex tasks - Tasks that require careful planning or multiple operations\n3. User explicitly requests todo list - When the user directly asks you to use the todo list\n4. User provides multiple tasks - When users provide a list of things to be done (numbered or comma-separated)\n5. After receiving new instructions - Immediately capture user requirements as todos\n6. When you start working on a task - Mark it as in_progress BEFORE beginning work. Ideally you should only have one todo as in_progress at a time\n7. After completing a task - Mark it as completed and add any new follow-up tasks discovered during implementation\n\n## When NOT to Use This Tool\n\nSkip using this tool when:\n1. There is only a single, straightforward task\n2. The task is trivial and tracking it provides no organizational benefit\n3. The task can be completed in less than 3 trivial steps\n4. The task is purely conversational or informational\n\nNOTE that you should not use this tool if there is only one trivial task to do. In this case you are better off just doing the task directly.\n\n## Examples of When to Use the Todo List\n\n<example>\nUser: I want to add a dark mode toggle to the application settings. Make sure you run the tests and build when you're done!\nAssistant: *Creates todo list with the following items:*\n1. Creating dark mode toggle component in Settings page\n2. Adding dark mode state management (context/store)\n3. Implementing CSS-in-JS styles for dark theme\n4. Updating existing components to support theme switching\n5. Running tests and build process, addressing any failures or errors that occur\n*Begins working on the first task*\n\n<reasoning>\nThe assistant used the todo list because:\n1. Adding dark mode is a multi-step feature requiring UI, state management, and styling changes\n2. The user explicitly requested tests and build be run afterward\n3. The assistant inferred that tests and build need to pass by adding \"Ensure tests and build succeed\" as the final task\n</reasoning>\n</example>\n\n<example>\nUser: Help me rename the function getCwd to getCurrentWorkingDirectory across my project\nAssistant: *Uses grep or search tools to locate all instances of getCwd in the codebase*\nI've found 15 instances of 'getCwd' across 8 different files.\n*Creates todo list with specific items for each file that needs updating*\n\n<reasoning>\nThe assistant used the todo list because:\n1. First, the assistant searched to understand the scope of the task\n2. Upon finding multiple occurrences across different files, it determined this was a complex task with multiple steps\n3. The todo list helps ensure every instance is tracked and updated systematically\n4. This approach prevents missing any occurrences and maintains code consistency\n</reasoning>\n</example>\n\n\n<example>\nUser: I need to implement these features for my e-commerce site: user registration, product catalog, shopping cart, and checkout flow.\nAssistant: *Creates a todo list breaking down each feature into specific tasks based on the project architecture*\nLet's start with user registration. This will involve creating database models, API endpoints, and frontend forms that integrate with your existing authentication system.\n\n<reasoning>\nThe assistant used the todo list because:\n1. The user provided multiple complex features to implement in a comma separated list\n2. The todo list helps organize these large features into manageable tasks\n3. This approach allows for tracking progress across the entire implementation\n</reasoning>\n</example>\n\n<example>\nUser: Can you help optimize my React application? It's rendering slowly and has performance issues.\nAssistant: *Reviews component structure, render patterns, state management, and data fetching*\nAfter analyzing your codebase, I've identified several performance issues.\n*Creates todo list with items like: 1) Implementing memoization for expensive calculations in ProductList, 2) Adding virtualization for long lists in Dashboard, 3) Optimizing image loading in Gallery component, 4) Fixing state update loops in ShoppingCart, 5) Reviewing bundle size and implementing code splitting*\nLet's start by implementing memoization for the expensive calculations in your ProductList component.\n\n<reasoning>\nThe assistant used the todo list because:\n1. First, the assistant examined the codebase to identify specific performance issues\n2. Based on this analysis, it identified multiple optimization opportunities\n3. Performance optimization is a non-trivial task requiring multiple steps\n4. The todo list helps methodically track improvements across different components\n5. This systematic approach ensures all performance bottlenecks are addressed\n</reasoning>\n</example>\n\n## Examples of When NOT to Use the Todo List\n\n<example>\nUser: How do I print 'Hello World' in Python?\nAssistant: In Python, you can print \"Hello World\" with this simple code:\n\npython\nprint(\"Hello World\")\n\nThis will output the text \"Hello World\" to the console when executed.\n\n<reasoning>\nThe assistant did not use the todo list because this is a single, trivial task that can be completed in one step. There's no need to track multiple tasks or steps for such a straightforward request.\n</reasoning>\n</example>\n\n<example>\nUser: What does the git status command do?\nAssistant: The git status command shows the current state of your working directory and staging area. It displays which changes have been staged, which haven't, and which files aren't being tracked by Git. This is a fundamental command for understanding the current state of your repository before making commits.\n\n<reasoning>\nThe assistant did not use the todo list because this is an informational request with no actual coding task to complete. The user is simply asking for an explanation, not for the assistant to perform multiple steps or tasks.\n</reasoning>\n</example>\n\n<example>\nUser: Can you add a comment to the calculateTotal function to explain what it does?\nAssistant: Sure, let me add a comment to the calculateTotal function to explain what it does.\n* Uses the Edit tool to add a comment to the calculateTotal function *\n\n<reasoning>\nThe assistant did not use the todo list because this is a single, straightforward task confined to one location in the code. Adding a comment doesn't require tracking multiple steps or systematic organization.\n</reasoning>\n</example>\n\n<example>\nUser: Run npm install for me and tell me what happens.\nAssistant: I'll run the npm install command for you.\n\n*Executes: npm install*\n\nThe command completed successfully. Here's the output:\n[Output of npm install command]\n\nAll dependencies have been installed according to your package.json file.\n\n<reasoning>\nThe assistant did not use the todo list because this is a single command execution with immediate results. There are no multiple steps to track or organize, making the todo list unnecessary for this straightforward task.\n</reasoning>\n</example>\n\n## Task States and Management\n\n1. **Task States**: Use these states to track progress:\n   - pending: Task not yet started\n   - in_progress: Currently working on (limit to ONE task at a time)\n   - completed: Task finished successfully\n\n   **IMPORTANT**: Task descriptions must have two forms:\n   - content: The imperative form describing what needs to be done (e.g., \"Run tests\", \"Build the project\")\n   - activeForm: The present continuous form shown during execution (e.g., \"Running tests\", \"Building the project\")\n\n2. **Task Management**:\n   - Update task status in real-time as you work\n   - Mark tasks complete IMMEDIATELY after finishing (don't batch completions)\n   - Exactly ONE task must be in_progress at any time (not less, not more)\n   - Complete current tasks before starting new ones\n   - Remove tasks that are no longer relevant from the list entirely\n\n3. **Task Completion Requirements**:\n   - ONLY mark a task as completed when you have FULLY accomplished it\n   - If you encounter errors, blockers, or cannot finish, keep the task as in_progress\n   - When blocked, create a new task describing what needs to be resolved\n   - Never mark a task as completed if:\n     - Tests are failing\n     - Implementation is partial\n     - You encountered unresolved errors\n     - You couldn't find necessary files or dependencies\n\n4. **Task Breakdown**:\n   - Create specific, actionable items\n   - Break complex tasks into smaller, manageable steps\n   - Use clear, descriptive task names\n   - Always provide both forms:\n     - content: \"Fix authentication bug\"\n     - activeForm: \"Fixing authentication bug\"\n\nWhen in doubt, use this tool. Being proactive with task management demonstrates attentiveness and ensures you complete all requirements successfully.";

#[test]
fn sleep_tool_is_visible_to_anthropic_and_openai_tool_builders() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let expected = reference_sleep_prompt();

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_sleep = anthropic
        .iter()
        .find(|definition| definition["name"] == json!("Sleep"))
        .expect("Sleep tool definition");
    assert_eq!(anthropic_sleep["description"], json!(expected.clone()));
    assert_eq!(
        anthropic_sleep["input_schema"]["required"],
        json!(["duration_ms"])
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_sleep = openai
        .iter()
        .find(|definition| definition.name == "Sleep")
        .expect("Sleep tool definition");
    assert_eq!(
        openai_sleep.description,
        expected_openai_tool_description("Sleep", "Sleep", &expected)
    );
    assert_eq!(openai_sleep.parameters["required"], json!(["duration_ms"]));
}

#[test]
fn bundled_resources_register_sleep_tool() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry.definition("Sleep").expect("Sleep tool definition");

    assert_eq!(definition.handler, "runtime:sleep");
    assert_eq!(definition.description, reference_sleep_prompt());
}

#[test]
fn openai_function_tool_parameters_use_valid_object_roots() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let tools = openai_tool_definitions(&registry, None, false).unwrap();

    for tool in tools.iter().filter(|tool| tool.kind == "function") {
        assert_eq!(
            tool.parameters
                .get("type")
                .and_then(serde_json::Value::as_str),
            Some("object"),
            "{} parameters must have an object root for OpenAI",
            tool.name
        );
        for keyword in ["oneOf", "anyOf", "allOf", "enum", "not"] {
            assert!(
                tool.parameters.get(keyword).is_none(),
                "{} parameters must not use top-level `{}` for OpenAI",
                tool.name,
                keyword
            );
        }
    }

    let mcp_tool = tools
        .iter()
        .find(|tool| tool.name == "McpToolCall")
        .expect("McpToolCall should be model-visible");
    assert_eq!(mcp_tool.parameters.get("oneOf"), None);
}

#[test]
fn notebook_edit_tool_is_visible_to_anthropic_and_openai_tool_builders() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_notebook = anthropic
        .iter()
        .find(|definition| definition["name"] == json!("NotebookEdit"))
        .expect("NotebookEdit tool definition");
    assert_eq!(
        anthropic_notebook["description"],
        json!(NOTEBOOK_EDIT_DESCRIPTION)
    );
    assert_eq!(
        anthropic_notebook["input_schema"]["required"],
        json!(["notebook_path", "new_source"])
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_notebook = openai
        .iter()
        .find(|definition| definition.name == "NotebookEdit")
        .expect("NotebookEdit tool definition");
    assert_eq!(
        openai_notebook.description,
        expected_openai_tool_description("NotebookEdit", "NotebookEdit", NOTEBOOK_EDIT_DESCRIPTION)
    );
    assert_eq!(
        openai_notebook.parameters["required"],
        json!(["notebook_path", "new_source"])
    );
}

#[test]
fn bundled_resources_register_notebook_edit_tool() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let definition = registry
        .definition("NotebookEdit")
        .expect("NotebookEdit tool definition");

    assert_eq!(definition.handler, "runtime:notebook_edit");
    assert_eq!(definition.description, NOTEBOOK_EDIT_DESCRIPTION);
    assert_eq!(definition.display.group.as_deref(), Some("files"));
    assert_eq!(definition.display.title.as_deref(), Some("NotebookEdit"));
    assert!(definition.display.show_in_status);
}

#[test]
fn workflow_tool_descriptions_match_claude_reference_for_anthropic_and_openai() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);

    for tool_id in [
        "AskUserQuestion",
        "EnterPlanMode",
        "ExitPlanMode",
        "Skill",
        "TodoWrite",
        "ToolSearch",
        "SendMessage",
        "SendUserMessage",
        "TaskGet",
        "TaskList",
        "TaskStop",
        "TaskOutput",
    ] {
        let description = registry
            .definition(tool_id)
            .expect("tool definition")
            .description
            .clone();

        let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
        let anthropic_definition = anthropic
            .iter()
            .find(|item| item["name"] == json!(tool_id))
            .expect("anthropic tool definition");
        assert_eq!(
            anthropic_definition["description"],
            json!(description.clone())
        );

        let openai = openai_tool_definitions(&registry, None, false).unwrap();
        let openai_definition = openai
            .iter()
            .find(|item| item.name == tool_id)
            .expect("openai tool definition");
        assert_eq!(
            openai_definition.description,
            expected_openai_tool_description(tool_id, tool_id, &description)
        );
    }
}

#[test]
fn agent_tool_description_is_rendered_for_anthropic_and_openai() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let description = registry
        .definition("Agent")
        .expect("Agent tool definition")
        .description
        .clone();

    for snippet in AGENT_TOOL_SNIPPETS {
        assert!(
            description.contains(snippet),
            "Agent description missing snippet: {snippet}"
        );
    }

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|item| item["name"] == json!("Agent"))
        .expect("anthropic Agent tool definition");
    assert_eq!(
        anthropic_definition["description"],
        json!(description.clone())
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_definition = openai
        .iter()
        .find(|item| item.name == "Agent")
        .expect("openai Agent tool definition");
    assert_eq!(
        openai_definition.description,
        expected_openai_tool_description("Agent", "Agent", &description)
    );
}

#[test]
fn config_tool_description_is_rendered_for_anthropic_and_openai() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let description = registry
        .definition("Config")
        .expect("Config tool definition")
        .description
        .clone();

    assert!(description.contains("### Global Settings"));
    assert!(description.contains("copy_full_response"));
    assert!(description.contains("### Project Settings"));
    assert!(description.contains("openai_headers"));
    assert!(description.contains("### Session Settings"));
    assert!(description.contains("statuslineEnabled"));

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|item| item["name"] == json!("Config"))
        .expect("anthropic Config tool definition");
    assert_eq!(
        anthropic_definition["description"],
        json!(description.clone())
    );
    assert!(
        anthropic_definition["input_schema"]["properties"]["value"]["oneOf"]
            .as_array()
            .is_some_and(|variants| variants.iter().any(|variant| variant["type"] == "integer"))
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_definition = openai
        .iter()
        .find(|item| item.name == "Config")
        .expect("openai Config tool definition");
    assert_eq!(
        openai_definition.description,
        expected_openai_tool_description("Config", "Config", &description)
    );
    assert!(openai_definition.parameters["properties"]["value"]["oneOf"]
        .as_array()
        .is_some_and(|variants| variants.iter().any(|variant| variant["type"] == "integer")));
}

#[test]
fn lsp_tool_description_matches_claude_reference_for_anthropic_and_openai() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    assert_tool_description_matches_expected(&registry, "LSP", reference_lsp_prompt().as_str());
}

#[test]
fn powershell_tool_description_matches_claude_reference_for_anthropic_and_openai() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let expected = reference_powershell_prompt();
    let description = registry
        .definition("PowerShell")
        .unwrap_or_else(|| {
            panic!(
                "PowerShell tool definition missing; available tools: {:?}",
                registry
                    .definitions()
                    .map(|definition| definition.id.clone())
                    .collect::<Vec<_>>()
            )
        })
        .description
        .clone();
    assert_eq!(
        normalize_prompt_lines(&description),
        normalize_prompt_lines(&expected)
    );

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|definition| definition["name"] == json!("PowerShell"))
        .expect("PowerShell anthropic tool definition");
    assert_eq!(
        normalize_prompt_lines(
            anthropic_definition["description"]
                .as_str()
                .expect("anthropic description"),
        ),
        normalize_prompt_lines(&expected)
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_definition = openai
        .iter()
        .find(|definition| definition.name == "PowerShell")
        .expect("PowerShell openai tool definition");
    assert_eq!(
        normalize_prompt_lines(&openai_definition.description),
        normalize_prompt_lines(&expected_openai_tool_description(
            "PowerShell",
            "PowerShell",
            &expected
        ))
    );
    assert_eq!(
        openai_definition.parameters["properties"]["timeout"]["description"],
        json!("Optional timeout in milliseconds (max 600000)")
    );
    assert_eq!(
        openai_definition.parameters["properties"]["run_in_background"]["description"],
        json!(
            "Set to true to run this command in the background. Use Read to read the output later."
        )
    );
    let mut property_names = openai_definition.parameters["properties"]
        .as_object()
        .expect("properties object")
        .keys()
        .map(String::as_str)
        .collect::<Vec<_>>();
    property_names.sort_unstable();
    assert_eq!(
        property_names,
        vec!["command", "description", "run_in_background", "timeout"]
    );
}

#[test]
fn web_search_tool_prompt_matches_claude_reference_for_anthropic_and_openai() {
    require_claude_reference!();
    let reference = read_repo_file("references/claude-code/src/tools/WebSearchTool/prompt.ts");
    let expected =
        normalize_reference_template(&extract_template_literal(&reference, "  return `"))
            .replace("${currentMonthYear}", &current_month_year());
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|definition| definition["name"] == json!("WebSearch"))
        .expect("WebSearch anthropic tool definition");
    assert_eq!(anthropic_definition["description"], json!(expected.clone()));

    // OpenAI now serializes WebSearch as a native server-side tool rather
    // than a function-shaped tool with a description. The function-shaped
    // fallback can still be opted into via PUFFER_OPENAI_NATIVE_WEB_SEARCH=0.
    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let native = openai
        .iter()
        .find(|definition| definition.kind == "web_search")
        .expect("WebSearch openai native tool definition");
    assert!(native.name.is_empty());
    assert!(native.description.is_empty());
    assert_eq!(
        serde_json::to_value(native).unwrap(),
        json!({ "type": "web_search", "external_web_access": true })
    );
    let _ = expected;
}

#[test]
fn selected_tool_prompts_match_claude_reference_for_anthropic_and_openai() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let openai = openai_tool_definitions(&registry, None, false).unwrap();

    for (tool_id, expected) in [
        ("TaskCreate", reference_task_create_prompt()),
        ("TaskUpdate", reference_task_update_prompt()),
        ("TeamCreate", reference_team_create_prompt()),
        ("TeamDelete", reference_team_delete_prompt()),
        ("EnterWorktree", reference_enter_worktree_prompt()),
        ("ExitWorktree", reference_exit_worktree_prompt()),
        (
            "ListMcpResourcesTool",
            reference_list_mcp_resources_prompt(),
        ),
        ("ReadMcpResourceTool", reference_read_mcp_resource_prompt()),
        ("ToolSearch", reference_tool_search_prompt()),
        ("WebFetch", reference_web_fetch_prompt()),
    ] {
        let definition = registry.definition(tool_id).expect("tool definition");
        assert_eq!(
            definition.description.trim_end(),
            expected.trim_end(),
            "registry description for {tool_id}"
        );

        let anthropic_definition = anthropic
            .iter()
            .find(|item| item["name"] == json!(tool_id))
            .expect("anthropic tool definition");
        assert_eq!(
            anthropic_definition["description"]
                .as_str()
                .expect("anthropic description")
                .trim_end(),
            expected.trim_end(),
            "anthropic description for {tool_id}"
        );

        let openai_definition = openai
            .iter()
            .find(|item| item.name == tool_id)
            .expect("openai tool definition");
        assert_eq!(
            openai_definition.description.trim_end(),
            expected_openai_tool_description(tool_id, tool_id, &expected).trim_end(),
            "openai description for {tool_id}"
        );
    }
}

#[test]
fn built_in_agent_resources_match_claude_reference_prompts() {
    require_claude_reference!();
    let resources = bundled_resources();

    for (agent_id, expected_description, expected_prompt) in [
        (
            "Explore",
            reference_explore_agent_description(),
            reference_explore_agent_prompt(),
        ),
        (
            "general-purpose",
            reference_general_purpose_agent_description(),
            reference_general_purpose_agent_prompt(),
        ),
        (
            "Plan",
            reference_plan_agent_description(),
            reference_plan_agent_prompt(),
        ),
        (
            "statusline-setup",
            reference_statusline_setup_agent_description(),
            reference_statusline_setup_agent_prompt(),
        ),
        (
            "verification",
            reference_verification_agent_description(),
            reference_verification_agent_prompt(),
        ),
    ] {
        let agent = resources
            .agents
            .iter()
            .find(|item| item.value.id == agent_id)
            .unwrap_or_else(|| panic!("missing builtin agent {agent_id}"));

        assert_eq!(
            normalize_inline_whitespace(&agent.value.description),
            normalize_inline_whitespace(&expected_description),
            "description for {agent_id}"
        );
        assert_eq!(
            normalize_agent_prompt_text(&agent.value.prompt),
            normalize_agent_prompt_text(&expected_prompt),
            "prompt for {agent_id}"
        );
    }
}

#[test]
fn file_tool_prompts_and_schemas_match_claude_reference_for_anthropic_and_openai() {
    require_claude_reference!();
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let openai = openai_tool_definitions(&registry, None, false).unwrap();

    for (tool_id, expected_prompt, expected_schema) in [
        (
            "Read",
            reference_file_read_prompt(),
            reference_file_read_schema(),
        ),
        (
            "Edit",
            reference_file_edit_prompt(),
            reference_file_edit_schema(),
        ),
        (
            "Write",
            reference_file_write_prompt(),
            reference_file_write_schema(),
        ),
    ] {
        let definition = registry.definition(tool_id).expect("tool definition");
        assert_eq!(
            trim_line_trailing_whitespace(&definition.description),
            trim_line_trailing_whitespace(&expected_prompt),
            "registry description for {tool_id}"
        );
        assert_eq!(
            definition.input_schema.as_json_schema(),
            expected_schema,
            "registry schema for {tool_id}"
        );

        let anthropic_definition = anthropic
            .iter()
            .find(|item| item["name"] == json!(tool_id))
            .expect("anthropic tool definition");
        assert_eq!(
            trim_line_trailing_whitespace(
                anthropic_definition["description"]
                    .as_str()
                    .expect("anthropic description"),
            ),
            trim_line_trailing_whitespace(&expected_prompt),
            "anthropic description for {tool_id}"
        );
        assert_eq!(
            anthropic_definition["input_schema"], expected_schema,
            "anthropic schema for {tool_id}"
        );

        let openai_definition = openai
            .iter()
            .find(|item| item.name == tool_id)
            .expect("openai tool definition");
        assert_eq!(
            trim_line_trailing_whitespace(&openai_definition.description),
            trim_line_trailing_whitespace(&expected_openai_tool_description(
                tool_id,
                tool_id,
                &expected_prompt
            )),
            "openai description for {tool_id}"
        );
        assert_eq!(
            openai_definition.parameters, expected_schema,
            "openai schema for {tool_id}"
        );
    }
}

#[test]
fn ask_user_question_schema_supports_multi_select_answers_for_both_providers() {
    let resources = bundled_resources();
    let registry = ToolRegistry::from_resources(&resources);
    let expected = json!([
        { "type": "string" },
        {
            "type": "array",
            "items": { "type": "string" }
        }
    ]);

    let anthropic = anthropic_tool_definitions(&registry, None).unwrap();
    let anthropic_definition = anthropic
        .iter()
        .find(|item| item["name"] == json!("AskUserQuestion"))
        .expect("anthropic AskUserQuestion tool definition");
    assert_eq!(
        anthropic_definition["input_schema"]["properties"]["answers"]["additionalProperties"]
            ["oneOf"],
        expected
    );

    let openai = openai_tool_definitions(&registry, None, false).unwrap();
    let openai_definition = openai
        .iter()
        .find(|item| item.name == "AskUserQuestion")
        .expect("openai AskUserQuestion tool definition");
    assert_eq!(
        openai_definition.parameters["properties"]["answers"]["additionalProperties"]["oneOf"],
        expected
    );
}
