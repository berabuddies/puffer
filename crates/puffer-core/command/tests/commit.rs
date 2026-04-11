use super::*;
use std::process::Command;
use tempfile::tempdir;

#[test]
fn commit_command_uses_reference_prompt_text_from_resources() {
    let tempdir = tempdir().unwrap();
    let repo = tempdir.path().join("repo");
    std::fs::create_dir_all(&repo).unwrap();
    assert!(Command::new("git")
        .arg("init")
        .arg(&repo)
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.name", "Test User"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["config", "user.email", "test@example.com"])
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("README.md"), "hello\n").unwrap();
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["add", "README.md"])
        .status()
        .unwrap()
        .success());
    assert!(Command::new("git")
        .arg("-C")
        .arg(&repo)
        .args(["commit", "-m", "init"])
        .status()
        .unwrap()
        .success());
    std::fs::write(repo.join("README.md"), "hello\nupdated\n").unwrap();

    let paths = ConfigPaths::discover(tempdir.path());
    ensure_workspace_dirs(&paths).unwrap();
    let session_store = SessionStore::from_paths(&paths).unwrap();
    let session = session_store.create_session(repo.clone()).unwrap();
    let mut state = AppState::new(PufferConfig::default(), repo, session);
    let resources = LoadedResources {
        prompts: vec![LoadedItem {
            value: serde_yaml::from_str::<PromptTemplate>(include_str!(
                "../../../../resources/prompts/commit.yaml"
            ))
            .unwrap(),
            source_info: SourceInfo {
                path: PathBuf::from("resources/prompts/commit.yaml"),
                kind: SourceKind::Builtin,
            },
        }],
        ..LoadedResources::default()
    };

    dispatch_command(
        &mut state,
        &supported_commands(),
        &resources,
        &mut ProviderRegistry::new(),
        &mut AuthStore::default(),
        &session_store,
        "/commit",
    )
    .unwrap();

    assert!(matches!(
        state.transcript.first(),
        Some(RenderedMessage {
            role: MessageRole::User,
            text, ..
        }) if text.contains("## Context")
            && text.contains("Current git status:")
            && text.contains("Current git diff (staged and unstaged changes):")
            && text.contains("README.md")
            && text.contains("git commit -m \"$(cat <<'EOF'")
            && !text.contains("Command mode:")
    ));
    assert!(matches!(
        state.transcript.last(),
        Some(RenderedMessage {
            role: MessageRole::System,
            text, ..
        }) if text.contains("Prompt command /commit failed")
    ));
}
