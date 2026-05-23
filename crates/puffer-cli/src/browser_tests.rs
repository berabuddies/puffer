use super::*;
use crate::browser_args::{
    BrowserCommand, BrowserKeyboardCommand, BrowserTabCommand, BrowserTargetArgs,
};
use crate::cli_args::{Cli, Command};
use clap::Parser;
use puffer_core::browser_action_set_for_action;

#[test]
fn resume_flag_without_value_uses_empty_sentinel() {
    let cli = Cli::parse_from(["puffer", "--resume"]);
    assert_eq!(cli.resume.as_deref(), Some(""));
    assert!(cli.prompt.is_none());
}

#[test]
fn resume_flag_with_value_keeps_positional_prompt() {
    let cli = Cli::parse_from(["puffer", "--resume", "dockyard", "follow up"]);
    assert_eq!(cli.resume.as_deref(), Some("dockyard"));
    assert_eq!(cli.prompt.as_deref(), Some("follow up"));
}

#[test]
fn remote_prompt_collects_trailing_words() {
    let cli = Cli::parse_from([
        "puffer",
        "remote",
        "c@localhost",
        "--cwd",
        "/tmp/demo",
        "hello",
        "from",
        "remote",
    ]);
    let Some(Command::Remote {
        target,
        cwd,
        no_alt_screen,
        prompt,
    }) = cli.subcommand
    else {
        panic!("expected remote command");
    };
    assert_eq!(target, "c@localhost");
    assert_eq!(cwd.as_deref(), Some("/tmp/demo"));
    assert!(!no_alt_screen);
    assert_eq!(prompt, ["hello", "from", "remote"]);
}

#[test]
fn ok_message_is_human_readable_for_common_actions() {
    assert_eq!(ok_message("reload"), "reloaded");
    assert_eq!(ok_message("select"), "selected option");
    assert_eq!(ok_message("upload"), "uploaded files");
    assert_eq!(ok_message("press"), "pressed key");
    assert_eq!(ok_message("scrollIntoView"), "scrolled into view");
}

#[test]
fn payload_helpers_skip_missing_values() {
    let mut payload = base_payload("snapshot", "session-123");
    apply_target_args(
        &mut payload,
        &BrowserTargetArgs {
            tab_id: None,
            width: Some(1200),
            height: None,
        },
    );
    assert_eq!(
        payload.get("sessionId").and_then(Value::as_str),
        Some("session-123")
    );
    assert_eq!(payload.get("width").and_then(Value::as_u64), Some(1200));
    assert!(payload.get("tabId").is_none());
    assert!(payload.get("height").is_none());
}

#[test]
fn snapshot_results_are_normalized_into_snapshot_and_refs() {
    let result = normalize_agent_result(
        BrowserPrintKind::Snapshot,
        serde_json::json!({
            "url": "https://example.com/form",
            "title": "Example Form",
            "text": "Name\nSave",
            "elements": [
                {
                    "ref": "@e1",
                    "role": "textbox",
                    "name": "Your name",
                    "tag": "textarea",
                    "href": null,
                    "x": 120.0,
                    "y": 220.0
                },
                {
                    "ref": "@e2",
                    "role": "button",
                    "name": "Save",
                    "tag": "button",
                    "href": null,
                    "x": 120.0,
                    "y": 260.0
                }
            ],
            "instruction": "Refs are fresh for this snapshot."
        }),
    )
    .expect("normalize snapshot");

    assert_eq!(
        result.get("origin").and_then(Value::as_str),
        Some("https://example.com/form")
    );
    assert_eq!(
        result.get("snapshot").and_then(Value::as_str),
        Some("Name\nSave")
    );
    assert!(result.get("elements").is_none());
    assert!(
        result.pointer("/refs/@e1/x").is_none(),
        "normalized refs should not expose raw coordinates"
    );
    assert_eq!(
        result.pointer("/refs/@e2/role").and_then(Value::as_str),
        Some("button")
    );
}

#[test]
fn render_snapshot_body_uses_origin_snapshot_and_refs_sections() {
    let snapshot = BrowserSnapshotOutput {
        origin: "https://example.com/form".to_string(),
        title: "Example Form".to_string(),
        snapshot: "Name\nSave".to_string(),
        refs: IndexMap::from([
            (
                "@e1".to_string(),
                BrowserSnapshotRef {
                    role: "textbox".to_string(),
                    name: "Your name".to_string(),
                    tag: "textarea".to_string(),
                    href: None,
                },
            ),
            (
                "@e2".to_string(),
                BrowserSnapshotRef {
                    role: "button".to_string(),
                    name: "Save".to_string(),
                    tag: "button".to_string(),
                    href: Some("https://example.com/save".to_string()),
                },
            ),
        ]),
        instruction: "Refs are fresh for this snapshot.".to_string(),
    };

    let rendered = render_snapshot_body(&snapshot);
    assert!(rendered.contains("origin: https://example.com/form"));
    assert!(rendered.contains("snapshot:\nName\nSave"));
    assert!(rendered.contains("refs:\n  @e1 textbox textarea \"Your name\""));
    assert!(rendered.contains("@e2 button button \"Save\" <https://example.com/save>"));
}

#[test]
fn redact_internal_fields_removes_backend_session_ids() {
    let value = redact_internal_fields(serde_json::json!({
        "tabId": "t1",
        "backendSessionId": "root:browser:t1",
        "tabs": [
            {
                "tabId": "t2",
                "backendSessionId": "root:browser:t2"
            }
        ]
    }));

    assert!(value.get("backendSessionId").is_none());
    assert!(value.pointer("/tabs/0/backendSessionId").is_none());
}

#[test]
fn browser_command_parses_global_json_and_session_flags() {
    let cli = Cli::parse_from([
        "puffer",
        "browser",
        "--json",
        "--session-id",
        "session-123",
        "list",
    ]);
    let Some(Command::Browser(args)) = cli.subcommand else {
        panic!("expected browser command");
    };
    assert!(args.json);
    assert_eq!(args.session_id.as_deref(), Some("session-123"));
}

#[test]
fn browser_snapshot_target_parse() {
    let cli = Cli::parse_from(["puffer", "browser", "snapshot", "--tab-id", "t2"]);
    let Some(Command::Browser(args)) = cli.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Snapshot { target } = args.command else {
        panic!("expected snapshot command");
    };
    assert_eq!(target.tab_id.as_deref(), Some("t2"));
}

#[test]
fn browser_screenshot_command_parses_artifact_flags() {
    let cli = Cli::parse_from([
        "puffer",
        "browser",
        "screenshot",
        "captures/page.jpeg",
        "--annotate",
        "--screenshot-dir",
        "ignored-dir",
        "--screenshot-format",
        "jpeg",
        "--screenshot-quality",
        "82",
        "--tab-id",
        "t4",
    ]);
    let Some(Command::Browser(args)) = cli.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Screenshot {
        path,
        annotate,
        screenshot_dir,
        screenshot_format,
        screenshot_quality,
        target,
    } = args.command
    else {
        panic!("expected screenshot command");
    };
    assert_eq!(path.unwrap().to_string_lossy(), "captures/page.jpeg");
    assert!(annotate);
    assert_eq!(screenshot_dir.unwrap().to_string_lossy(), "ignored-dir");
    assert_eq!(screenshot_format.as_deref(), Some("jpeg"));
    assert_eq!(screenshot_quality, Some(82));
    assert_eq!(target.tab_id.as_deref(), Some("t4"));
}

#[test]
fn browser_tab_focus_and_select_parse_tab_id() {
    let cli = Cli::parse_from(["puffer", "browser", "tab", "focus", "t4"]);
    let Some(Command::Browser(args)) = cli.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Tab { command } = args.command else {
        panic!("expected tab command");
    };
    let BrowserTabCommand::Focus { tab_id } = command else {
        panic!("expected tab focus command");
    };
    assert_eq!(tab_id, "t4");

    let alias = Cli::parse_from(["puffer", "browser", "tab", "select", "t5"]);
    let Some(Command::Browser(args)) = alias.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Tab { command } = args.command else {
        panic!("expected tab command");
    };
    let BrowserTabCommand::Focus { tab_id } = command else {
        panic!("expected tab select alias");
    };
    assert_eq!(tab_id, "t5");
}

#[test]
fn browser_aliases_parse_for_goto_key_and_exit() {
    let goto = Cli::parse_from(["puffer", "browser", "goto", "https://example.com"]);
    let Some(Command::Browser(args)) = goto.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Navigate { .. }));

    let key = Cli::parse_from(["puffer", "browser", "key", "Enter"]);
    let Some(Command::Browser(args)) = key.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Press { .. }));

    let exit = Cli::parse_from(["puffer", "browser", "exit"]);
    let Some(Command::Browser(args)) = exit.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Quit));
}

#[test]
fn browser_keyboard_and_scroll_commands_parse() {
    let keyboard = Cli::parse_from(["puffer", "browser", "keyboard", "insert-text", "hello"]);
    let Some(Command::Browser(args)) = keyboard.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(
        args.command,
        BrowserCommand::Keyboard {
            command: BrowserKeyboardCommand::InsertText { .. }
        }
    ));

    let scroll = Cli::parse_from(["puffer", "browser", "scrollinto", "@e7"]);
    let Some(Command::Browser(args)) = scroll.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(
        args.command,
        BrowserCommand::ScrollIntoView { .. }
    ));
}

#[test]
fn browser_focus_commands_parse() {
    let focus = Cli::parse_from(["puffer", "browser", "focus", "@e3", "--tab-id", "t3"]);
    let Some(Command::Browser(args)) = focus.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Focus { ref_id, target } = args.command else {
        panic!("expected focus command");
    };
    assert_eq!(ref_id, "@e3");
    assert_eq!(target.tab_id.as_deref(), Some("t3"));

    let focus_ref = Cli::parse_from(["puffer", "browser", "focus-ref", "@e4"]);
    let Some(Command::Browser(args)) = focus_ref.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Focus { .. }));
}

#[test]
fn browser_select_and_toggle_commands_parse() {
    let select = Cli::parse_from(["puffer", "browser", "select", "@e4", "New York"]);
    let Some(Command::Browser(args)) = select.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Select { ref_id, value, .. } = args.command else {
        panic!("expected select command");
    };
    assert_eq!(ref_id, "@e4");
    assert_eq!(value, "New York");

    let check = Cli::parse_from(["puffer", "browser", "check", "@e8"]);
    let Some(Command::Browser(args)) = check.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Check { .. }));

    let uncheck = Cli::parse_from(["puffer", "browser", "uncheck", "@e8"]);
    let Some(Command::Browser(args)) = uncheck.subcommand else {
        panic!("expected browser command");
    };
    assert!(matches!(args.command, BrowserCommand::Uncheck { .. }));
}

#[test]
fn browser_upload_command_parses_multiple_files_and_target() {
    let cli = Cli::parse_from([
        "puffer",
        "browser",
        "upload",
        "@e9",
        "fixtures/one.txt",
        "fixtures/two.txt",
        "--tab-id",
        "t6",
    ]);
    let Some(Command::Browser(args)) = cli.subcommand else {
        panic!("expected browser command");
    };
    let BrowserCommand::Upload {
        ref_id,
        files,
        target,
    } = args.command
    else {
        panic!("expected upload command");
    };
    assert_eq!(ref_id, "@e9");
    assert_eq!(
        files,
        vec![
            PathBuf::from("fixtures/one.txt"),
            PathBuf::from("fixtures/two.txt")
        ]
    );
    assert_eq!(target.tab_id.as_deref(), Some("t6"));
}

#[test]
fn browser_cli_actions_share_core_action_set_mapping() {
    assert_eq!(
        browser_action_set_for_action("focus_ref").map(|set| format!("{set:?}")),
        Some("Interact".to_string())
    );
}

#[test]
fn upload_files_are_canonicalized_to_absolute_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let file = tempdir.path().join("upload-one.txt");
    std::fs::write(&file, "one").expect("write upload file");
    let canonical = file.canonicalize().expect("canonicalize upload file");

    let paths = canonicalize_upload_files(tempdir.path(), &[PathBuf::from("upload-one.txt")])
        .expect("canonicalize upload files");

    assert_eq!(paths, vec![canonical.to_string_lossy().into_owned()]);
}

#[test]
fn upload_files_reject_missing_or_non_file_paths() {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let directory = tempdir.path().join("nested");
    std::fs::create_dir(&directory).expect("create directory");

    let missing = canonicalize_upload_files(tempdir.path(), &[PathBuf::from("missing.txt")])
        .expect_err("missing file should fail");
    assert!(missing
        .to_string()
        .contains("browser upload file not found"));

    let not_file = canonicalize_upload_files(tempdir.path(), &[PathBuf::from("nested")])
        .expect_err("directory should fail");
    assert!(not_file
        .to_string()
        .contains("browser upload path is not a file"));
}
