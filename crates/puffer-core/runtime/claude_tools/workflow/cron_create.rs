use crate::AppState;
use anyhow::{bail, Context, Result};
use serde_json::json;
use serde_json::Value;
use std::path::Path;
use uuid::Uuid;

use super::store::{
    crons_path, load_store, save_store, validate_cron_expression, CronCreateInput, CronStore,
    StoredCronJob,
};

/// Executes the Claude-compatible `CronCreate` workflow tool.
pub fn execute_cron_create(state: &mut AppState, _cwd: &Path, input: Value) -> Result<String> {
    let parsed: CronCreateInput =
        serde_json::from_value(input).context("invalid CronCreate input")?;
    validate_cron_expression(&parsed.cron)?;
    if parsed.prompt.trim().is_empty() {
        bail!("CronCreate prompt cannot be empty");
    }
    let mut store = load_store::<CronStore>(&crons_path(state.session.cwd.as_path()))?;
    let job = StoredCronJob {
        id: format!("cron-{}", Uuid::new_v4().simple()),
        cron: parsed.cron,
        prompt: parsed.prompt,
        recurring: parsed.recurring,
        durable: parsed.durable,
    };
    store.jobs.push(job.clone());
    save_store(&crons_path(state.session.cwd.as_path()), &store)?;
    Ok(serde_json::to_string_pretty(&job).unwrap_or_else(|_| json!({}).to_string()))
}
