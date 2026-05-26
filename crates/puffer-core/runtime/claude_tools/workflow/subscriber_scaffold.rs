//! `SubscriberScaffold` workflow tool — drops a new subscriber skill
//! directory under `~/.puffer/subscribers/<id>/` containing a starter
//! manifest, a stub `run` script in the chosen language, and a README
//! describing the ndjson protocol the agent must implement.
//!
//! The agent then edits the `run` script to implement the actual
//! polling / event-emission logic, and finally calls `SubscriberInstall`
//! to start it. This separation keeps installation behind an explicit
//! user-approval step.

use crate::AppState;
use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct ScaffoldInput {
    /// Stable kebab-case id; becomes the directory name and default topic.
    id: String,
    /// Free-form description, written into the README so future readers
    /// know why the subscriber exists.
    description: String,
    /// Language for the stub `run` script: `"python"`, `"sh"`, or
    /// `"node"`. The scaffold writes a working "hello world" skeleton
    /// in that language; the agent fills in the actual logic afterward.
    language: String,
    /// Optional topic override. Defaults to `id`.
    #[serde(default)]
    topic: Option<String>,
}

/// Executes `SubscriberScaffold`. Creates the skill directory tree and
/// returns a JSON object listing the files written so the agent can
/// edit `run` in the next turn.
pub fn execute_subscriber_scaffold(
    _state: &mut AppState,
    _cwd: &Path,
    input: Value,
) -> Result<String> {
    let parsed: ScaffoldInput =
        serde_json::from_value(input).context("invalid SubscriberScaffold input")?;
    if !is_valid_id(&parsed.id) {
        bail!(
            "id `{}` must be lowercase kebab-case (a-z, 0-9, -)",
            parsed.id
        );
    }

    let template = ScriptTemplate::for_language(&parsed.language)?;
    let dir = subscribers_root()?.join(&parsed.id);
    if dir.exists() {
        bail!(
            "subscriber `{}` already exists at {}; pick a different id or delete the existing dir first",
            parsed.id,
            dir.display()
        );
    }
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create subscriber dir {}", dir.display()))?;

    let topic = parsed.topic.clone().unwrap_or_else(|| parsed.id.clone());
    let manifest = render_manifest(&parsed.id, &topic, &template);
    let manifest_path = dir.join("manifest.toml");
    std::fs::write(&manifest_path, manifest)
        .with_context(|| format!("write {}", manifest_path.display()))?;

    let run_path = dir.join(template.filename);
    std::fs::write(&run_path, template.body)
        .with_context(|| format!("write {}", run_path.display()))?;
    set_executable(&run_path);

    let readme_path = dir.join("README.md");
    std::fs::write(&readme_path, render_readme(&parsed.id, &parsed.description))
        .with_context(|| format!("write {}", readme_path.display()))?;

    Ok(json!({
        "dir": dir.display().to_string(),
        "files": {
            "manifest": manifest_path.display().to_string(),
            "run": run_path.display().to_string(),
            "readme": readme_path.display().to_string(),
        },
        "next": "Edit the `run` script to implement the subscriber. Then call `SubscriberInstall` with this id to start it.",
    })
    .to_string())
}

fn is_valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
        && !id.ends_with('-')
}

fn subscribers_root() -> Result<PathBuf> {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .ok_or_else(|| anyhow!("HOME is not set; cannot place subscriber skill"))?;
    let dir = home.join(".puffer").join("subscribers");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("create subscribers root {}", dir.display()))?;
    Ok(dir)
}

struct ScriptTemplate {
    filename: &'static str,
    body: String,
}

impl ScriptTemplate {
    fn for_language(language: &str) -> Result<Self> {
        match language {
            "python" | "py" => Ok(Self {
                filename: "run",
                body: PYTHON_TEMPLATE.to_string(),
            }),
            "sh" | "bash" => Ok(Self {
                filename: "run",
                body: SH_TEMPLATE.to_string(),
            }),
            "node" | "js" | "javascript" => Ok(Self {
                filename: "run",
                body: NODE_TEMPLATE.to_string(),
            }),
            other => Err(anyhow!(
                "language `{other}` not supported; use one of python|sh|node"
            )),
        }
    }
}

fn render_manifest(id: &str, topic: &str, _template: &ScriptTemplate) -> String {
    format!(
        "manifest_version = 1\n\
         id = \"{id}\"\n\
         kind = \"subscriber\"\n\
         topic = \"{topic}\"\n\
         display_name = \"{id}\"\n\
         \n\
         [run]\n\
         cmd = [\"./run\"]\n\
         \n\
         [state]\n\
         dir = \"state\"\n"
    )
}

fn render_readme(id: &str, description: &str) -> String {
    format!(
        "# {id}\n\n\
         {description}\n\n\
         ## Protocol\n\n\
         This is a Puffer subscriber skill. The supervisor runs `./run`\n\
         with stdin/stdout piped:\n\n\
         * **stdout**: write one JSON value per line. Each line must match\n\
           `{{ topic, kind, dedup_key?, text, payload }}` where `topic`\n\
           defaults to this skill's id, `kind` is a short tag like\n\
           `\"message\"`, `dedup_key` is a stable id for de-dup across\n\
           restarts, `text` is what the subscription router runs regex\n\
           and LLM judges against, and `payload` is arbitrary JSON.\n\
         * **stdin**: optionally read one JSON value per line for control\n\
           messages. Ignore unrecognized variants.\n\
         * **stderr**: log freely; the supervisor mirrors it through\n\
           tracing.\n\n\
         ## State\n\n\
         Persistent state goes under `$PUFFER_SKILL_STATE_DIR` (the\n\
         supervisor creates the directory). Use it for last-seen ids,\n\
         offsets, etc.\n"
    )
}

#[cfg(unix)]
fn set_executable(path: &Path) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o111);
        let _ = std::fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_executable(_path: &Path) {}

const PYTHON_TEMPLATE: &str = r#"#!/usr/bin/env python3
"""Subscriber stub. Emits ndjson events on stdout, reads commands on stdin."""
import json, os, sys, time

TOPIC = os.environ.get("PUFFER_SKILL_TOPIC", "subscriber")
STATE_DIR = os.environ.get("PUFFER_SKILL_STATE_DIR", "./state")


def emit(kind, dedup_key, text, payload):
    line = json.dumps({
        "topic": TOPIC,
        "kind": kind,
        "dedup_key": dedup_key,
        "text": text,
        "payload": payload,
    })
    sys.stdout.write(line + "\n")
    sys.stdout.flush()


def poll_once():
    # TODO: replace with your polling logic.
    # Example: emit("message", "demo:1", "hello world", {"source": "demo"})
    pass


def main():
    while True:
        try:
            poll_once()
        except Exception as exc:  # pragma: no cover
            print(f"poll error: {exc}", file=sys.stderr)
        time.sleep(60)


if __name__ == "__main__":
    main()
"#;

const SH_TEMPLATE: &str = r#"#!/usr/bin/env bash
# Subscriber stub. Emits ndjson events on stdout.
set -euo pipefail
TOPIC="${PUFFER_SKILL_TOPIC:-subscriber}"
STATE_DIR="${PUFFER_SKILL_STATE_DIR:-./state}"

emit() {
    # Args: kind dedup_key text payload_json
    printf '{"topic":"%s","kind":"%s","dedup_key":"%s","text":%s,"payload":%s}\n' \
        "$TOPIC" "$1" "$2" "$(jq -Rn --arg t "$3" '$t')" "$4"
}

while true; do
    # TODO: replace with your polling logic.
    # Example: emit message demo:1 "hello world" '{"source":"demo"}'
    sleep 60
done
"#;

const NODE_TEMPLATE: &str = r#"#!/usr/bin/env node
// Subscriber stub. Emits ndjson events on stdout, reads commands on stdin.
const TOPIC = process.env.PUFFER_SKILL_TOPIC || 'subscriber';
const STATE_DIR = process.env.PUFFER_SKILL_STATE_DIR || './state';

function emit(kind, dedupKey, text, payload) {
    const line = JSON.stringify({
        topic: TOPIC,
        kind,
        dedup_key: dedupKey,
        text,
        payload,
    });
    process.stdout.write(line + '\n');
}

async function pollOnce() {
    // TODO: replace with your polling logic.
    // Example: emit('message', 'demo:1', 'hello world', { source: 'demo' });
}

async function main() {
    for (;;) {
        try {
            await pollOnce();
        } catch (err) {
            console.error('poll error:', err);
        }
        await new Promise((r) => setTimeout(r, 60000));
    }
}

main();
"#;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_validation() {
        assert!(is_valid_id("rss-hn"));
        assert!(is_valid_id("github3"));
        assert!(!is_valid_id("Bad"));
        assert!(!is_valid_id("-leading"));
        assert!(!is_valid_id("trailing-"));
        assert!(!is_valid_id(""));
    }

    #[test]
    fn manifest_renders_with_topic_and_id() {
        let template = ScriptTemplate::for_language("python").unwrap();
        let m = render_manifest("rss-hn", "rss-hn", &template);
        assert!(m.contains("id = \"rss-hn\""));
        assert!(m.contains("topic = \"rss-hn\""));
        assert!(m.contains("cmd = [\"./run\"]"));
    }

    #[test]
    fn unknown_language_rejected() {
        let err = match ScriptTemplate::for_language("brainfuck") {
            Ok(_) => panic!("expected an error for unknown language"),
            Err(error) => error,
        };
        assert!(err.to_string().contains("not supported"));
    }
}
