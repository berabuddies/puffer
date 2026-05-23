use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

use crate::runtime::process_store::ProcessStore;
use std::sync::Mutex;

const MIN_YIELD_MS: u64 = 250;
const MAX_YIELD_MS: u64 = 30_000;
const DEFAULT_YIELD_MS: u64 = 5_000;

#[derive(Debug, Clone, Deserialize)]
pub struct WriteStdinInput {
    pub process_id: i32,
    #[serde(default, alias = "chars")]
    pub input: String,
    #[serde(default)]
    pub yield_time_ms: Option<u64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct WriteStdinOutput {
    #[serde(skip_serializing_if = "Option::is_none", rename = "processId")]
    pub process_id: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none", rename = "exitCode")]
    pub exit_code: Option<i32>,
    pub output: String,
}

pub fn execute(store: &Mutex<ProcessStore>, input: Value) -> Result<(bool, String)> {
    let args: WriteStdinInput =
        serde_json::from_value(input).context("invalid WriteStdin input")?;

    let yield_ms = args
        .yield_time_ms
        .unwrap_or(DEFAULT_YIELD_MS)
        .clamp(MIN_YIELD_MS, MAX_YIELD_MS);

    let output_baseline;
    {
        let mut guard = store.lock().unwrap();
        let Some(entry) = guard.get_mut(args.process_id) else {
            let out = WriteStdinOutput {
                process_id: None,
                exit_code: None,
                output: format!(
                    "process {} not found (may have exited and been cleaned up)",
                    args.process_id
                ),
            };
            return Ok((false, serde_json::to_string_pretty(&out)?));
        };

        if !args.input.is_empty() {
            entry
                .write_stdin(args.input.as_bytes())
                .with_context(|| format!("failed to write stdin to process {}", args.process_id))?;
        }

        output_baseline = entry.total_output_bytes();
    }

    let deadline = Instant::now() + Duration::from_millis(yield_ms);
    std::thread::sleep(Duration::from_millis(100.min(yield_ms)));

    loop {
        {
            let guard = store.lock().unwrap();
            if let Some(entry) = guard.peek(args.process_id) {
                if entry.total_output_bytes() > output_baseline || entry.has_exited() {
                    break;
                }
            } else {
                break;
            }
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let mut guard = store.lock().unwrap();
    let (alive, exit_code, text) = if let Some(entry) = guard.get_mut(args.process_id) {
        let new_output = entry.collect_output_since(output_baseline);
        let text = String::from_utf8_lossy(&new_output).to_string();
        let exited = entry.has_exited();
        let code = entry.exit_code();
        if exited {
            guard.remove(args.process_id);
        }
        (!exited, code, text)
    } else {
        (false, None, String::new())
    };

    let out = WriteStdinOutput {
        process_id: if alive { Some(args.process_id) } else { None },
        exit_code,
        output: text,
    };

    Ok((true, serde_json::to_string_pretty(&out)?))
}
