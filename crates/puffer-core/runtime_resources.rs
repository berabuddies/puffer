use anyhow::{anyhow, bail, Context, Result};
use puffer_config::{ConfigPaths, RemotePathMapConfig, RemoteToolRunnerConfig};
use puffer_remote_tools::load_project_resources_blocking;
use puffer_resources::{
    load_resources_for_runtime, load_resources_for_runtime_with_extra_roots, LoadedResources,
    RuntimeResourceRoot, SourceKind,
};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};

/// Loads runtime resources for one workspace, optionally augmenting them with
/// remote project resources fetched through the configured tool runner.
pub fn load_runtime_resources_for_paths(
    paths: &ConfigPaths,
    remote_tool_runner: Option<&RemoteToolRunnerConfig>,
) -> Result<LoadedResources> {
    let Some(config) = remote_tool_runner else {
        return load_resources_for_runtime(paths, false);
    };
    match load_remote_project_resource_roots(paths, config) {
        Ok(extra_roots) => {
            load_resources_for_runtime_with_extra_roots(paths, true, &extra_roots, &[])
        }
        Err(error) => load_resources_for_runtime_with_extra_roots(
            paths,
            true,
            &[],
            &[format!(
                "failed to load remote project resources: {error:#}"
            )],
        ),
    }
}

fn load_remote_project_resource_roots(
    paths: &ConfigPaths,
    config: &RemoteToolRunnerConfig,
) -> Result<Vec<RuntimeResourceRoot>> {
    let endpoint = config
        .endpoint
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| anyhow!("remote tool runner endpoint is not configured"))?;
    let auth_token = resolve_auth_token(config)?;
    let remote_workspace_root = resolve_remote_workspace_root(paths, config);
    let cache_root = remote_resource_cache_root(paths, endpoint, &remote_workspace_root);
    if cache_root.exists() {
        fs::remove_dir_all(&cache_root)
            .with_context(|| format!("failed to clear {}", cache_root.display()))?;
    }
    fs::create_dir_all(&cache_root)
        .with_context(|| format!("failed to create {}", cache_root.display()))?;
    let files = load_project_resources_blocking(
        endpoint,
        auth_token.as_deref(),
        remote_workspace_root.display().to_string().as_str(),
    )?;
    for file in files {
        let destination = destination_path(&cache_root, &file.relative_path)?;
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)
                .with_context(|| format!("failed to create {}", parent.display()))?;
        }
        fs::write(&destination, file.content)
            .with_context(|| format!("failed to write {}", destination.display()))?;
    }
    Ok(vec![
        RuntimeResourceRoot {
            filesystem_root: cache_root.join("resources"),
            logical_root: remote_workspace_root.join("resources"),
            kind: SourceKind::Builtin,
        },
        RuntimeResourceRoot {
            filesystem_root: cache_root.join(".puffer/resources"),
            logical_root: remote_workspace_root.join(".puffer/resources"),
            kind: SourceKind::Workspace,
        },
    ])
}

fn resolve_auth_token(config: &RemoteToolRunnerConfig) -> Result<Option<String>> {
    if let Some(token) = config
        .auth_token
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(token.to_string()));
    }
    if let Some(name) = config
        .auth_token_env
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        let value = std::env::var(name)
            .with_context(|| format!("failed to read tool runner token env `{name}`"))?;
        if value.trim().is_empty() {
            bail!("tool runner token env `{name}` is empty");
        }
        return Ok(Some(value));
    }
    Ok(None)
}

fn resolve_remote_workspace_root(paths: &ConfigPaths, config: &RemoteToolRunnerConfig) -> PathBuf {
    if let Some(explicit) = config
        .remote_cwd
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return PathBuf::from(explicit);
    }
    if let Some(mapped) =
        map_with_explicit_path_map(&paths.workspace_root, config.path_map.as_ref())
    {
        return mapped;
    }
    paths.workspace_root.clone()
}

fn map_with_explicit_path_map(path: &Path, map: Option<&RemotePathMapConfig>) -> Option<PathBuf> {
    let map = map?;
    let local_root = map.local_root.as_deref()?;
    let remote_root = map.remote_root.as_deref()?;
    path.strip_prefix(local_root)
        .ok()
        .map(|suffix| PathBuf::from(remote_root).join(suffix))
}

fn remote_resource_cache_root(
    paths: &ConfigPaths,
    endpoint: &str,
    remote_workspace_root: &Path,
) -> PathBuf {
    let mut digest = Sha256::new();
    digest.update(endpoint.as_bytes());
    digest.update([0]);
    digest.update(remote_workspace_root.display().to_string().as_bytes());
    let fingerprint = format!("{:x}", digest.finalize());
    paths
        .user_config_dir
        .join("runtime/remote_project_resources")
        .join(&fingerprint[..16])
}

fn destination_path(cache_root: &Path, relative_path: &str) -> Result<PathBuf> {
    let relative_path = Path::new(relative_path);
    if relative_path.is_absolute() {
        bail!(
            "remote resource path `{}` must be relative",
            relative_path.display()
        );
    }
    for component in relative_path.components() {
        match component {
            Component::Normal(_) | Component::CurDir => {}
            Component::ParentDir => {
                bail!(
                    "remote resource path `{}` must not escape the cache root",
                    relative_path.display()
                )
            }
            Component::RootDir | Component::Prefix(_) => {
                bail!(
                    "remote resource path `{}` must stay within the cache root",
                    relative_path.display()
                )
            }
        }
    }
    Ok(cache_root.join(relative_path))
}

#[cfg(test)]
mod tests {
    use super::*;
    use puffer_tool_runner::ToolRunnerService;
    use std::sync::mpsc;
    use std::thread;
    use tempfile::tempdir;
    use tokio::runtime::Builder;
    use tokio::sync::oneshot;
    use tokio_stream::wrappers::TcpListenerStream;

    struct TestRunnerHandle {
        endpoint: String,
        token: String,
        shutdown: Option<oneshot::Sender<()>>,
        thread: Option<thread::JoinHandle<()>>,
    }

    impl Drop for TestRunnerHandle {
        fn drop(&mut self) {
            if let Some(shutdown) = self.shutdown.take() {
                let _ = shutdown.send(());
            }
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn spawn_test_runner() -> TestRunnerHandle {
        let token = "test-remote-tool-runner-token".to_string();
        let service = ToolRunnerService::new(token.clone());
        let (ready_tx, ready_rx) = mpsc::channel();
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let thread = thread::spawn(move || {
            let runtime = Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("build tokio runtime for remote tool runner");
            runtime.block_on(async move {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                    .await
                    .expect("bind remote tool runner listener");
                let addr = listener
                    .local_addr()
                    .expect("read remote tool runner local address");
                ready_tx
                    .send(format!("http://{addr}"))
                    .expect("publish remote tool runner endpoint");
                tonic::transport::Server::builder()
                    .add_service(service.into_service())
                    .serve_with_incoming_shutdown(TcpListenerStream::new(listener), async {
                        let _ = shutdown_rx.await;
                    })
                    .await
                    .expect("serve remote tool runner");
            });
        });
        TestRunnerHandle {
            endpoint: ready_rx
                .recv()
                .expect("receive remote tool runner endpoint"),
            token,
            shutdown: Some(shutdown_tx),
            thread: Some(thread),
        }
    }

    #[test]
    fn load_runtime_resources_for_paths_loads_remote_project_resources() {
        let runner = spawn_test_runner();
        let temp = tempdir().unwrap();
        let local_root = temp.path().join("local");
        let remote_root = temp.path().join("remote");
        let user_root = temp.path().join("home/.puffer");

        fs::create_dir_all(local_root.join("resources/prompts")).unwrap();
        fs::create_dir_all(remote_root.join("resources/prompts")).unwrap();
        fs::create_dir_all(remote_root.join(".puffer/resources/skills/remote-review")).unwrap();
        fs::create_dir_all(user_root.join("resources/prompts")).unwrap();
        fs::write(
            local_root.join("resources/prompts/local-only.yaml"),
            "id: local-only\ndescription: Local\ntemplate: local\n",
        )
        .unwrap();
        fs::write(
            remote_root.join("resources/prompts/remote-only.yaml"),
            "id: remote-only\ndescription: Remote\ntemplate: remote\n",
        )
        .unwrap();
        fs::write(
            remote_root.join(".puffer/resources/skills/remote-review/SKILL.md"),
            "---\nname: remote-review\ndescription: Remote review\n---\nBody\n",
        )
        .unwrap();
        fs::write(
            user_root.join("resources/prompts/user-extra.yaml"),
            "id: user-extra\ndescription: User\ntemplate: user\n",
        )
        .unwrap();

        let paths = ConfigPaths {
            workspace_root: local_root.clone(),
            workspace_config_dir: local_root.join(".puffer"),
            user_config_dir: user_root,
            builtin_resources_dir: local_root.join("resources"),
        };
        let config = RemoteToolRunnerConfig {
            endpoint: Some(runner.endpoint.clone()),
            auth_token: Some(runner.token.clone()),
            auth_token_env: None,
            remote_cwd: Some(remote_root.display().to_string()),
            path_map: None,
        };

        let loaded = load_runtime_resources_for_paths(&paths, Some(&config)).unwrap();
        assert!(loaded
            .prompts
            .iter()
            .any(|prompt| prompt.value.id == "remote-only" && prompt.value.template == "remote"));
        assert!(loaded
            .prompts
            .iter()
            .any(|prompt| prompt.value.id == "user-extra"));
        assert!(!loaded
            .prompts
            .iter()
            .any(|prompt| prompt.value.id == "local-only"));
        let remote_prompt = loaded
            .prompts
            .iter()
            .find(|prompt| prompt.value.id == "remote-only")
            .unwrap();
        assert_eq!(
            remote_prompt.source_info.path,
            remote_root.join("resources/prompts/remote-only.yaml")
        );
        assert_eq!(remote_prompt.source_info.kind, SourceKind::Builtin);
        let remote_skill = puffer_resources::skill_by_name(&loaded, "remote-review").unwrap();
        assert_eq!(
            remote_skill.source_info.path,
            remote_root.join(".puffer/resources/skills/remote-review/SKILL.md")
        );
        assert_eq!(remote_skill.source_info.kind, SourceKind::Workspace);
    }
}
