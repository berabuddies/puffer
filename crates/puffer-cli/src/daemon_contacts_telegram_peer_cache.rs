//! Telegram peer-cache helpers for contact ranking.

use super::{
    merge_candidate_last_message_at_ms, merge_telegram_name,
    read_telegram_primary_peer_metadata_from_account, Candidate,
};
use anyhow::{Context, Result};
use grammers_session::Session;
use puffer_config::ConfigPaths;
use puffer_subscriber_telegram_user::{
    default_init_params, hydrate_contact_book_cache, hydrate_recent_dialog_peer_cache,
    resolve_api_credentials, Client, Config, PersistedCredentials, SkillEnv,
};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

const RECENT_DIALOG_CACHE_FILE: &str = "recent-dialog-cache.json";
const RECENT_DIALOG_TARGET_MIN: usize = 5;
const RECENT_DIALOG_TARGET_MAX: usize = 50;
const RECENT_DIALOG_MAX_DIALOGS: usize = 500;

const TELEGRAM_PEER_CACHE_HYDRATE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub(super) enum TelegramPeerCacheHydrationMode {
    IfNeeded,
    Force,
}

pub(super) fn collect_telegram_peer_cache_candidates(
    account_dir: &Path,
    by_id: &mut HashMap<String, Candidate>,
) {
    for (id, metadata) in read_telegram_primary_peer_metadata_from_account(account_dir) {
        let entry = by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id,
            name: metadata.name.clone(),
            avatar: metadata.avatar.clone(),
            score: 0.01,
            last_message_at_ms: metadata.last_message_at_ms,
            context: Vec::new(),
        });
        entry.score = entry.score.max(0.01);
        merge_candidate_last_message_at_ms(
            &mut entry.last_message_at_ms,
            metadata.last_message_at_ms,
        );
        merge_telegram_name(&mut entry.name, &metadata.name);
        if entry.avatar.is_none() {
            entry.avatar = metadata.avatar;
        }
    }
}

pub(super) fn hydrate_telegram_peer_cache_if_needed(paths: &ConfigPaths, account_dir: &Path) {
    hydrate_telegram_peer_cache(paths, account_dir, TelegramPeerCacheHydrationMode::IfNeeded);
}

pub(super) fn hydrate_telegram_peer_cache(
    paths: &ConfigPaths,
    account_dir: &Path,
    mode: TelegramPeerCacheHydrationMode,
) {
    if mode == TelegramPeerCacheHydrationMode::IfNeeded
        && !telegram_peer_cache_needs_hydration(account_dir)
    {
        return;
    }
    if let Err(error) = hydrate_telegram_peer_cache_from_session_blocking(paths, account_dir) {
        warn!(
            account = %account_dir.display(),
            %error,
            force = mode == TelegramPeerCacheHydrationMode::Force,
            "failed to hydrate Telegram peer cache for contacts list"
        );
    }
}

pub(super) fn hydrate_telegram_recent_peer_cache_if_needed(
    paths: &ConfigPaths,
    account_dir: &Path,
    limit: usize,
) {
    if telegram_recent_dialog_cache_ready(account_dir) {
        return;
    }
    if !account_dir.join("telegram.session").exists() {
        return;
    }
    if let Err(error) =
        hydrate_telegram_recent_peer_cache_from_session_blocking(paths, account_dir, limit)
    {
        warn!(
            account = %account_dir.display(),
            %error,
            "failed to hydrate Telegram recent dialog cache for contacts list"
        );
    }
}

pub(super) fn telegram_recent_dialog_cache_ready(account_dir: &Path) -> bool {
    let path = account_dir.join(RECENT_DIALOG_CACHE_FILE);
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    serde_json::from_str::<Value>(&raw)
        .ok()
        .and_then(|marker| marker.get("ready").and_then(Value::as_bool))
        .unwrap_or(false)
}

fn telegram_peer_cache_needs_hydration(account_dir: &Path) -> bool {
    if !account_dir.join("telegram.session").exists() {
        return false;
    }
    let path = account_dir.join("peer-cache.json");
    let Ok(raw) = std::fs::read_to_string(&path) else {
        return true;
    };
    let Ok(cache) = serde_json::from_str::<Value>(&raw) else {
        return true;
    };
    cache
        .get("peers")
        .and_then(Value::as_array)
        .map_or(true, Vec::is_empty)
}

fn hydrate_telegram_peer_cache_from_session_blocking(
    paths: &ConfigPaths,
    account_dir: &Path,
) -> Result<()> {
    #[cfg(test)]
    if let Some(result) = TEST_HYDRATOR.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|hydrator| hydrator(paths, account_dir))
    }) {
        return result;
    }

    let (sender, receiver) = mpsc::channel();
    let worker_paths = paths.clone();
    let worker_account_dir = account_dir.to_path_buf();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build Telegram contact hydrate runtime")
            .and_then(|runtime| {
                runtime.block_on(hydrate_telegram_peer_cache_from_session(
                    &worker_paths,
                    &worker_account_dir,
                ))
            });
        let _ = sender.send(result);
    });
    wait_for_telegram_peer_cache_hydrate_result(receiver, TELEGRAM_PEER_CACHE_HYDRATE_TIMEOUT)
}

fn wait_for_telegram_peer_cache_hydrate_result(
    receiver: Receiver<Result<()>>,
    timeout: Duration,
) -> Result<()> {
    match receiver.recv_timeout(timeout) {
        Ok(result) => result,
        Err(RecvTimeoutError::Timeout) => anyhow::bail!(
            "Telegram contact hydrate timed out after {}s",
            timeout.as_secs_f32()
        ),
        Err(RecvTimeoutError::Disconnected) => {
            anyhow::bail!("Telegram contact hydrate thread ended without a result")
        }
    }
}

async fn hydrate_telegram_peer_cache_from_session(
    paths: &ConfigPaths,
    account_dir: &Path,
) -> Result<()> {
    let env = telegram_skill_env(paths, account_dir);
    if !env.session_path.exists() {
        return Ok(());
    }
    let session = Session::load_file(&env.session_path)
        .with_context(|| format!("load Telegram session {}", env.session_path.display()))?;
    if !session.signed_in() {
        return Ok(());
    }
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = resolve_api_credentials(None, None, &persisted)?;
    let client = Client::connect(Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("connect Telegram contact hydrate client")?;
    if !client
        .is_authorized()
        .await
        .context("check Telegram contact hydrate authorization")?
    {
        return Ok(());
    }
    hydrate_contact_book_cache(&env, &client)
        .await
        .context("hydrate Telegram contact book cache")?;
    client
        .session()
        .save_to_file(&env.session_path)
        .with_context(|| format!("save Telegram session {}", env.session_path.display()))?;
    Ok(())
}

fn hydrate_telegram_recent_peer_cache_from_session_blocking(
    paths: &ConfigPaths,
    account_dir: &Path,
    limit: usize,
) -> Result<()> {
    let (sender, receiver) = mpsc::channel();
    let paths = paths.clone();
    let account_dir = account_dir.to_path_buf();
    std::thread::spawn(move || {
        let result = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build Telegram recent dialog hydrate runtime")
            .and_then(|runtime| {
                runtime.block_on(hydrate_telegram_recent_peer_cache_from_session(
                    &paths,
                    &account_dir,
                    limit,
                ))
            });
        let _ = sender.send(result);
    });
    wait_for_telegram_peer_cache_hydrate_result(receiver, TELEGRAM_PEER_CACHE_HYDRATE_TIMEOUT)
}

async fn hydrate_telegram_recent_peer_cache_from_session(
    paths: &ConfigPaths,
    account_dir: &Path,
    limit: usize,
) -> Result<()> {
    let env = telegram_skill_env(paths, account_dir);
    if !env.session_path.exists() {
        return Ok(());
    }
    let session = Session::load_file(&env.session_path)
        .with_context(|| format!("load Telegram session {}", env.session_path.display()))?;
    if !session.signed_in() {
        return Ok(());
    }
    let persisted = PersistedCredentials::load(&env.credentials_path()).unwrap_or_default();
    let (api_id, api_hash) = resolve_api_credentials(None, None, &persisted)?;
    let client = Client::connect(Config {
        session,
        api_id,
        api_hash,
        params: default_init_params(),
    })
    .await
    .context("connect Telegram recent dialog hydrate client")?;
    if !client
        .is_authorized()
        .await
        .context("check Telegram recent dialog hydrate authorization")?
    {
        return Ok(());
    }
    let target = limit
        .max(RECENT_DIALOG_TARGET_MIN)
        .min(RECENT_DIALOG_TARGET_MAX);
    let direct_users_seen =
        hydrate_recent_dialog_peer_cache(&env, &client, target, RECENT_DIALOG_MAX_DIALOGS)
            .await
            .context("hydrate Telegram recent dialog peer cache")?;
    client
        .session()
        .save_to_file(&env.session_path)
        .with_context(|| format!("save Telegram session {}", env.session_path.display()))?;
    write_recent_dialog_cache_marker(account_dir, direct_users_seen, target)?;
    info!(
        account = %account_dir.display(),
        direct_users_seen,
        target,
        "Telegram recent dialog cache ready"
    );
    Ok(())
}

fn write_recent_dialog_cache_marker(
    account_dir: &Path,
    direct_users_seen: usize,
    target: usize,
) -> Result<()> {
    std::fs::create_dir_all(account_dir)
        .with_context(|| format!("create Telegram account dir {}", account_dir.display()))?;
    let path = account_dir.join(RECENT_DIALOG_CACHE_FILE);
    let tmp = path.with_extension("tmp");
    std::fs::write(
        &tmp,
        serde_json::to_vec_pretty(&json!({
            "ready": true,
            "hydrated_at_ms": now_unix_millis(),
            "direct_users_seen": direct_users_seen,
            "target": target,
        }))?,
    )
    .with_context(|| format!("write {}", tmp.display()))?;
    std::fs::rename(&tmp, &path)
        .with_context(|| format!("rename {} -> {}", tmp.display(), path.display()))
}

fn now_unix_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

fn telegram_skill_env(paths: &ConfigPaths, account_dir: &Path) -> SkillEnv {
    let topic = account_dir
        .file_name()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .unwrap_or("telegram-user")
        .to_string();
    SkillEnv {
        state_dir: account_dir.to_path_buf(),
        session_path: account_dir.join("telegram.session"),
        topic,
        workspace_config_dir: Some(paths.workspace_config_dir.clone()),
        // Hydration reads the live session directly; nothing is staged.
        live_session_path: None,
    }
}

#[cfg(test)]
type TestHydrator = Box<dyn Fn(&ConfigPaths, &Path) -> Result<()> + 'static>;

#[cfg(test)]
thread_local! {
    static TEST_HYDRATOR: std::cell::RefCell<Option<TestHydrator>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
struct TestTelegramPeerCacheHydratorGuard;

#[cfg(test)]
impl Drop for TestTelegramPeerCacheHydratorGuard {
    fn drop(&mut self) {
        TEST_HYDRATOR.with(|cell| {
            *cell.borrow_mut() = None;
        });
    }
}

#[cfg(test)]
pub(super) fn install_test_telegram_peer_cache_hydrator<F>(hydrator: F) -> impl Drop
where
    F: Fn(&ConfigPaths, &Path) -> Result<()> + 'static,
{
    TEST_HYDRATOR.with(|cell| {
        *cell.borrow_mut() = Some(Box::new(hydrator));
    });
    TestTelegramPeerCacheHydratorGuard
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc;
    use std::time::Duration;

    #[test]
    fn telegram_peer_cache_hydrate_wait_times_out() {
        let (_sender, receiver) = mpsc::channel::<Result<()>>();

        let error = wait_for_telegram_peer_cache_hydrate_result(receiver, Duration::from_millis(0))
            .unwrap_err()
            .to_string();

        assert!(
            error.contains("timed out"),
            "unexpected timeout error: {error}"
        );
    }
}
