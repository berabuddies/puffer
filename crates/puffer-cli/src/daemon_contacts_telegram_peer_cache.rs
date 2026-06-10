//! Telegram peer-cache helpers for contact ranking.

use super::{merge_telegram_name, read_telegram_peer_metadata_from_account, Candidate};
use anyhow::{Context, Result};
use grammers_session::Session;
use puffer_config::ConfigPaths;
use puffer_subscriber_telegram_user::{
    default_init_params, hydrate_contact_book_cache, resolve_api_credentials, Client, Config,
    PersistedCredentials, SkillEnv,
};
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use tracing::warn;

pub(super) fn collect_telegram_peer_cache_candidates(
    account_dir: &Path,
    by_id: &mut HashMap<String, Candidate>,
) {
    for (id, metadata) in read_telegram_peer_metadata_from_account(account_dir) {
        let entry = by_id.entry(id.clone()).or_insert_with(|| Candidate {
            id,
            name: metadata.name.clone(),
            avatar: metadata.avatar.clone(),
            score: 0.01,
            context: Vec::new(),
        });
        entry.score = entry.score.max(0.01);
        merge_telegram_name(&mut entry.name, &metadata.name);
        if entry.avatar.is_none() {
            entry.avatar = metadata.avatar;
        }
    }
}

pub(super) fn hydrate_telegram_peer_cache_if_needed(paths: &ConfigPaths, account_dir: &Path) {
    if !telegram_peer_cache_needs_hydration(account_dir) {
        return;
    }
    if let Err(error) = hydrate_telegram_peer_cache_from_session_blocking(paths, account_dir) {
        warn!(
            account = %account_dir.display(),
            %error,
            "failed to hydrate Telegram peer cache for contacts list"
        );
    }
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
    let paths = paths.clone();
    let account_dir = account_dir.to_path_buf();
    std::thread::spawn(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .context("build Telegram contact hydrate runtime")?;
        runtime.block_on(hydrate_telegram_peer_cache_from_session(
            &paths,
            &account_dir,
        ))
    })
    .join()
    .map_err(|_| anyhow::anyhow!("Telegram contact hydrate thread panicked"))?
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
    }
}
