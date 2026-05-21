use puffer_config::{ensure_workspace_dirs, ConfigPaths};
use puffer_provider_registry::{OAuthCredential, StoredCredential};
use puffer_session_store::SessionMetadata;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use uuid::Uuid;

#[test]
fn top_level_help_shows_claude_style_public_surface() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "agents",
        "auth",
        "auto-mode",
        "browser",
        "doctor",
        "install",
        "mcp",
        "plugin",
        "setup-token",
        "update",
    ] {
        assert!(
            stdout.contains(command),
            "missing `{command}` in help:\n{stdout}"
        );
    }
    assert!(
        stdout.contains("--resume"),
        "missing `--resume` in help:\n{stdout}"
    );
    for hidden in [
        "anthropic-request-fixture",
        "providers",
        "resources",
        "sessions",
        "tool",
    ] {
        assert!(
            !stdout.contains(hidden),
            "unexpected `{hidden}` in help:\n{stdout}"
        );
    }
}

#[test]
fn auth_help_hides_internal_subcommands_and_logout_clears_tokens() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let help = run_puffer(&workspace, &puffer_home, &["auth", "--help"]);
    assert!(help.status.success());
    let help_text = String::from_utf8_lossy(&help.stdout);
    assert!(help_text.contains("status"));
    assert!(help_text.contains("login"));
    assert!(help_text.contains("logout"));
    assert!(!help_text.contains("set-api-key"));
    assert!(!help_text.contains("oauth-start"));

    let setup = run_puffer(
        &workspace,
        &puffer_home,
        &["setup-token", "anthropic", "sk-test-token"],
    );
    assert!(setup.status.success(), "{setup:?}");

    let status = run_puffer(&workspace, &puffer_home, &["auth", "status", "--text"]);
    assert!(status.status.success(), "{status:?}");
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(status_text.contains("anthropic (api_key)"), "{status_text}");

    let logout = run_puffer(&workspace, &puffer_home, &["auth", "logout", "anthropic"]);
    assert!(logout.status.success(), "{logout:?}");
    let logout_text = String::from_utf8_lossy(&logout.stdout);
    assert!(logout_text.contains("cleared credentials for anthropic"));

    let status = run_puffer(&workspace, &puffer_home, &["auth", "status", "--text"]);
    assert!(status.status.success(), "{status:?}");
    let status_text = String::from_utf8_lossy(&status.stdout);
    assert!(status_text.contains("No providers are authenticated."));
}

#[test]
fn resume_help_mentions_the_tui() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["resume", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Resume a stored session in the TUI"));
}

#[test]
fn startup_resume_picker_does_not_create_blank_session() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    write_session_fixture(
        &puffer_home,
        Uuid::parse_str("00000000-0000-0000-0000-000000000101").unwrap(),
        "dockyard alpha",
        &workspace,
        10,
    );
    write_session_fixture(
        &puffer_home,
        Uuid::parse_str("00000000-0000-0000-0000-000000000102").unwrap(),
        "dockyard beta",
        &workspace,
        20,
    );
    let before = session_fixture_count(&puffer_home);

    let output = run_puffer(&workspace, &puffer_home, &["--resume", "dockyard"]);

    assert!(!output.status.success(), "{output:?}");
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("interactive TUI requires stdin and stdout to be terminals"),
        "{stderr}"
    );
    assert_eq!(session_fixture_count(&puffer_home), before);
}

#[test]
fn remote_help_mentions_ssh_launch() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["remote", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Launch Puffer on a remote host over SSH"));
    assert!(stdout.contains("<TARGET>"));
    assert!(stdout.contains("--cwd"));
}

#[test]
fn non_interactive_startup_prompt_prints_command_output() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["--no-alt-screen", "/status"]);
    assert!(output.status.success(), "{output:?}");
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Status"), "{stdout}");
    assert!(stdout.contains("Provider:"), "{stdout}");
}

#[test]
fn browser_help_lists_phase_two_surface_commands() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["browser", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in [
        "list",
        "open",
        "navigate",
        "back",
        "forward",
        "reload",
        "close",
        "quit",
        "tab",
        "snapshot",
        "screenshot",
        "click",
        "select",
        "upload",
        "check",
        "uncheck",
        "fill",
        "type",
        "press",
        "eval",
    ] {
        assert!(
            stdout.contains(command),
            "missing `{command}` in browser help:\n{stdout}"
        );
    }
    assert!(
        !stdout.contains("window"),
        "unexpected `window` in browser help:\n{stdout}"
    );
}

#[test]
fn browser_tab_help_lists_new_subcommand() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["browser", "tab", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    for command in ["list", "new", "close"] {
        assert!(
            stdout.contains(command),
            "missing `{command}` in browser tab help:\n{stdout}"
        );
    }
}

#[test]
fn desktop_api_help_is_hidden_but_available() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let output = run_puffer(&workspace, &puffer_home, &["desktop-api", "--help"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("login-api-key"));
    assert!(stdout.contains("logout"));
    assert!(stdout.contains("session-groups"));
    assert!(stdout.contains("session-detail"));
    assert!(stdout.contains("settings-snapshot"));
}

#[test]
fn desktop_api_auth_round_trip_updates_snapshot() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let before = run_puffer(
        &workspace,
        &puffer_home,
        &["desktop-api", "settings-snapshot"],
    );
    assert!(before.status.success(), "{before:?}");
    let before_text = String::from_utf8_lossy(&before.stdout);
    assert!(before_text.contains("\"auth\": []"), "{before_text}");

    let login = run_puffer(
        &workspace,
        &puffer_home,
        &[
            "desktop-api",
            "login-api-key",
            "anthropic",
            "--api-key",
            "sk-test-desktop-api",
        ],
    );
    assert!(login.status.success(), "{login:?}");
    let login_text = String::from_utf8_lossy(&login.stdout);
    assert!(
        login_text.contains("\"providerId\": \"anthropic\""),
        "{login_text}"
    );
    assert!(login_text.contains("\"kind\": \"api_key\""), "{login_text}");

    let logout = run_puffer(
        &workspace,
        &puffer_home,
        &["desktop-api", "logout", "anthropic"],
    );
    assert!(logout.status.success(), "{logout:?}");
    let logout_text = String::from_utf8_lossy(&logout.stdout);
    assert!(logout_text.contains("\"auth\": []"), "{logout_text}");
}

#[test]
fn desktop_api_store_oauth_credential_updates_snapshot() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();
    let credential_json = serde_json::to_string(&StoredCredential::OAuth(OAuthCredential {
        access_token: "access-token".to_string(),
        refresh_token: "refresh-token".to_string(),
        expires_at_ms: 1_700_000_000_000,
        email: Some("remote@example.com".to_string()),
        scopes: vec!["openid".to_string()],
        ..OAuthCredential::default()
    }))
    .expect("serialize oauth credential");

    let store = run_puffer(
        &workspace,
        &puffer_home,
        &[
            "desktop-api",
            "store-credential",
            "openai",
            "--credential-json",
            &credential_json,
        ],
    );
    assert!(store.status.success(), "{store:?}");
    let store_text = String::from_utf8_lossy(&store.stdout);
    assert!(
        store_text.contains("\"providerId\": \"openai\""),
        "{store_text}"
    );
    assert!(store_text.contains("\"kind\": \"oauth\""), "{store_text}");
    assert!(
        store_text.contains("\"email\": \"remote@example.com\""),
        "{store_text}"
    );
}

#[test]
fn mcp_round_trip_add_get_list_remove() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let add = run_puffer(
        &workspace,
        &puffer_home,
        &[
            "mcp",
            "add",
            "demo",
            "https://example.invalid/mcp",
            "--transport",
            "http",
        ],
    );
    assert!(add.status.success(), "{add:?}");

    let get = run_puffer(&workspace, &puffer_home, &["mcp", "get", "demo"]);
    assert!(get.status.success(), "{get:?}");
    let get_text = String::from_utf8_lossy(&get.stdout);
    assert!(get_text.contains("\"id\": \"demo\""));
    assert!(get_text.contains("\"transport\": \"http\""));

    let get_plugin = run_puffer(
        &workspace,
        &puffer_home,
        &["mcp", "get", "puffer-builtins:filesystem"],
    );
    assert!(get_plugin.status.success(), "{get_plugin:?}");
    let get_plugin_text = String::from_utf8_lossy(&get_plugin.stdout);
    assert!(get_plugin_text.contains("\"id\": \"puffer-builtins:filesystem\""));
    assert!(get_plugin_text.contains("\"plugin_id\": \"puffer-builtins\""));

    let list = run_puffer(&workspace, &puffer_home, &["mcp", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("demo [http]"));

    let remove = run_puffer(&workspace, &puffer_home, &["mcp", "remove", "demo"]);
    assert!(remove.status.success(), "{remove:?}");

    let list = run_puffer(&workspace, &puffer_home, &["mcp", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(!list_text.contains("demo [http]"), "{list_text}");
}

#[test]
fn plugin_round_trip_install_disable_enable_validate_uninstall() {
    let (tempdir, workspace, puffer_home) = configured_workspace();
    let manifest_path = tempdir.path().join("demo-plugin.yaml");
    fs::write(
        &manifest_path,
        r#"id: demo
display_name: Demo Plugin
description: Demo plugin
commands:
  - name: demo
    description: Demo command
"#,
    )
    .expect("plugin manifest");

    let validate = run_puffer(
        &workspace,
        &puffer_home,
        &["plugin", "validate", manifest_path.to_str().unwrap()],
    );
    assert!(validate.status.success(), "{validate:?}");
    let validate_text = String::from_utf8_lossy(&validate.stdout);
    assert!(validate_text.contains("\"id\": \"demo\""));

    let install = run_puffer(
        &workspace,
        &puffer_home,
        &["plugin", "install", manifest_path.to_str().unwrap()],
    );
    assert!(install.status.success(), "{install:?}");

    let disable = run_puffer(&workspace, &puffer_home, &["plugin", "disable", "demo"]);
    assert!(disable.status.success(), "{disable:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("demo [disabled]"), "{list_text}");

    let enable = run_puffer(&workspace, &puffer_home, &["plugin", "enable", "demo"]);
    assert!(enable.status.success(), "{enable:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("demo [enabled]"), "{list_text}");

    let uninstall = run_puffer(&workspace, &puffer_home, &["plugin", "uninstall", "demo"]);
    assert!(uninstall.status.success(), "{uninstall:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(!list_text.contains("demo [enabled]"), "{list_text}");
    assert!(!list_text.contains("demo [disabled]"), "{list_text}");
}

#[test]
fn builtin_plugin_disable_and_update_keep_disabled_state() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let disable = run_puffer(&workspace, &puffer_home, &["plugin", "disable", "example"]);
    assert!(disable.status.success(), "{disable:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("example [disabled]"), "{list_text}");

    let update = run_puffer(&workspace, &puffer_home, &["plugin", "update", "example"]);
    assert!(update.status.success(), "{update:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("example [disabled]"), "{list_text}");

    let enable = run_puffer(&workspace, &puffer_home, &["plugin", "enable", "example"]);
    assert!(enable.status.success(), "{enable:?}");

    let list = run_puffer(&workspace, &puffer_home, &["plugin", "list"]);
    assert!(list.status.success(), "{list:?}");
    let list_text = String::from_utf8_lossy(&list.stdout);
    assert!(list_text.contains("example [enabled]"), "{list_text}");
}

#[test]
fn install_and_update_commands_fail_when_not_implemented() {
    let (_tempdir, workspace, puffer_home) = configured_workspace();

    let install = run_puffer(&workspace, &puffer_home, &["install", "latest"]);
    assert!(!install.status.success(), "{install:?}");
    let install_text = String::from_utf8_lossy(&install.stderr);
    assert!(
        install_text.contains("does not ship a self-installer"),
        "{install_text}"
    );

    let update = run_puffer(&workspace, &puffer_home, &["update"]);
    assert!(!update.status.success(), "{update:?}");
    let update_text = String::from_utf8_lossy(&update.stderr);
    assert!(
        update_text.contains("does not include a self-updater"),
        "{update_text}"
    );
}

fn configured_workspace() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let tempdir = tempfile::tempdir().expect("tempdir");
    let workspace = tempdir.path().join("workspace");
    let puffer_home = tempdir.path().join("puffer-home");
    fs::create_dir_all(&workspace).expect("workspace");
    fs::create_dir_all(&puffer_home).expect("puffer-home");
    let paths = ConfigPaths::discover(&workspace);
    ensure_workspace_dirs(&paths).expect("dirs");
    let repo_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("cli crate parent")
        .parent()
        .expect("repo root");
    std::os::unix::fs::symlink(repo_root.join("resources"), workspace.join("resources"))
        .expect("resource symlink");
    (tempdir, workspace, puffer_home)
}

fn write_session_fixture(
    puffer_home: &Path,
    id: Uuid,
    display_name: &str,
    cwd: &Path,
    updated_at_ms: u64,
) {
    let root = puffer_home.join(".puffer").join("sessions");
    fs::create_dir_all(&root).expect("sessions dir");
    let metadata = SessionMetadata {
        id,
        display_name: Some(display_name.to_string()),
        generated_title: None,
        cwd: cwd.to_path_buf(),
        created_at_ms: 1,
        updated_at_ms,
        parent_session_id: None,
        slug: Some(display_name.replace(' ', "-")),
        tags: Vec::new(),
        note: None,
    };
    let body = serde_json::json!({ "metadata": metadata });
    fs::write(
        root.join(format!("{id}.session.json")),
        serde_json::to_vec(&body).expect("session json"),
    )
    .expect("write session");
    fs::write(root.join(format!("{id}.jsonl")), b"").expect("write jsonl");
}

fn session_fixture_count(puffer_home: &Path) -> usize {
    let root = puffer_home.join(".puffer").join("sessions");
    fs::read_dir(root)
        .expect("session dir")
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .path()
                .file_name()
                .and_then(|value| value.to_str())
                .is_some_and(|name| name.ends_with(".session.json"))
        })
        .count()
}

fn run_puffer(workspace: &Path, puffer_home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_puffer"))
        .args(args)
        .current_dir(workspace)
        .env("PUFFER_HOME", puffer_home)
        .output()
        .expect("puffer process")
}
