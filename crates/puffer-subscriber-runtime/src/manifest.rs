//! Subscriber manifest schema and loader.
//!
//! A subscriber directory looks like:
//!
//! ```text
//! <skills_root>/subscribers/<id>/
//!   manifest.toml
//!   run            # or any executable; referenced by manifest.run.cmd
//! ```
//!
//! Example `manifest.toml`:
//!
//! ```toml
//! manifest_version = 1
//! id = "telegram-user"
//! kind = "subscriber"
//! topic = "telegram-user"
//! display_name = "Telegram (user account)"
//!
//! [run]
//! cmd = ["puffer", "__subscriber", "telegram-user"]
//!
//! [state]
//! dir = "state"             # relative to manifest dir; runtime creates it
//! ```

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// Errors returned by manifest discovery and parsing.
#[derive(Debug, Error)]
pub enum ManifestError {
    /// The manifest file could not be read.
    #[error("failed to read manifest {path}: {source}")]
    Read {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    /// The manifest file was not valid TOML or had an unknown shape.
    #[error("failed to parse manifest {path}: {source}")]
    Parse {
        path: PathBuf,
        #[source]
        source: toml::de::Error,
    },
    /// The manifest declared a field outside the supported range.
    #[error("unsupported manifest: {message}")]
    Unsupported { message: String },
}

/// What the manifest describes. Only `subscriber` is wired up today;
/// `action` is reserved for future expansion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ManifestKind {
    /// A process that produces events onto the bus.
    Subscriber,
    /// A process invoked per-event to apply a side effect.
    Action,
}

/// Parsed `manifest.toml`, plus the absolute directory that contained it.
///
/// Callers typically only need [`Manifest::run_cmd`] and
/// [`Manifest::state_dir`]; the other fields are retained for diagnostics and
/// later UI.
#[derive(Debug, Clone)]
pub struct Manifest {
    /// Absolute path to the directory containing `manifest.toml`.
    pub dir: PathBuf,
    /// Parsed manifest body.
    pub spec: ManifestSpec,
}

/// Raw on-disk manifest fields.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ManifestSpec {
    /// Schema version. Only `1` is currently supported.
    pub manifest_version: u32,
    /// Stable id (filesystem-safe); matches the subscriber's default
    /// topic and the directory name.
    pub id: String,
    /// What the manifest describes.
    pub kind: ManifestKind,
    /// Topic subscribers publish on; defaults to `id` when absent.
    #[serde(default)]
    pub topic: Option<String>,
    /// Human-readable label for UI.
    #[serde(default)]
    pub display_name: Option<String>,
    /// The command the runtime should spawn.
    pub run: RunSpec,
    /// Optional state-directory declaration.
    #[serde(default)]
    pub state: Option<StateSpec>,
}

/// How the runtime launches the subscriber's executable.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RunSpec {
    /// argv: `cmd[0]` is the program, `cmd[1..]` are arguments.
    pub cmd: Vec<String>,
    /// Extra environment variables merged on top of the puffer process env.
    #[serde(default)]
    pub env: Vec<EnvEntry>,
}

/// Environment variable entry for [`RunSpec::env`].
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct EnvEntry {
    /// Variable name.
    pub name: String,
    /// Variable value (literal; no interpolation).
    pub value: String,
}

/// Optional state-directory declaration. The runtime creates `dir` (relative
/// to the manifest dir, unless absolute) before spawning the child and passes
/// it as `PUFFER_SKILL_STATE_DIR`.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct StateSpec {
    /// Directory path. Relative paths are resolved against the manifest dir.
    pub dir: String,
}

impl Manifest {
    /// Reads `<dir>/manifest.toml` and returns the parsed manifest.
    pub fn load(dir: impl AsRef<Path>) -> Result<Self, ManifestError> {
        let dir = dir.as_ref().to_path_buf();
        let path = dir.join("manifest.toml");
        let raw = std::fs::read_to_string(&path).map_err(|source| ManifestError::Read {
            path: path.clone(),
            source,
        })?;
        let spec: ManifestSpec =
            toml::from_str(&raw).map_err(|source| ManifestError::Parse { path, source })?;
        if spec.manifest_version != 1 {
            return Err(ManifestError::Unsupported {
                message: format!(
                    "manifest {}/manifest.toml: unsupported manifest_version {}",
                    dir.display(),
                    spec.manifest_version
                ),
            });
        }
        if spec.run.cmd.is_empty() {
            return Err(ManifestError::Unsupported {
                message: format!(
                    "manifest {}/manifest.toml: run.cmd must not be empty",
                    dir.display()
                ),
            });
        }
        Ok(Self { dir, spec })
    }

    /// Returns the effective topic (spec `topic` if set, else `id`).
    pub fn topic(&self) -> &str {
        self.spec.topic.as_deref().unwrap_or(self.spec.id.as_str())
    }

    /// Returns the state directory path (creates it on demand if declared).
    pub fn ensure_state_dir(&self) -> std::io::Result<Option<PathBuf>> {
        let Some(state) = &self.spec.state else {
            return Ok(None);
        };
        let path = if Path::new(&state.dir).is_absolute() {
            PathBuf::from(&state.dir)
        } else {
            self.dir.join(&state.dir)
        };
        std::fs::create_dir_all(&path)?;
        Ok(Some(path))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[test]
    fn parses_minimal_subscriber_manifest() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.toml"),
            r#"manifest_version = 1
id = "telegram-user"
kind = "subscriber"

[run]
cmd = ["puffer", "__subscriber", "telegram-user"]
"#,
        )
        .unwrap();
        let manifest = Manifest::load(dir.path()).unwrap();
        assert_eq!(manifest.spec.id, "telegram-user");
        assert_eq!(manifest.topic(), "telegram-user");
        assert_eq!(manifest.spec.kind, ManifestKind::Subscriber);
    }

    #[test]
    fn rejects_empty_cmd() {
        let dir = tempdir().unwrap();
        std::fs::write(
            dir.path().join("manifest.toml"),
            r#"manifest_version = 1
id = "bad"
kind = "subscriber"

[run]
cmd = []
"#,
        )
        .unwrap();
        let err = Manifest::load(dir.path()).unwrap_err();
        assert!(matches!(err, ManifestError::Unsupported { .. }));
    }
}
