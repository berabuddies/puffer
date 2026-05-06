use super::{BrowserCommand, BrowserKeyboardCommand, BrowserTabCommand, Cli, Command};
use clap::Parser;

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
    assert!(matches!(args.command, BrowserCommand::ScrollIntoView { .. }));
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
