//! Runtime staging helpers for built-in browser extensions.

use anyhow::{bail, Context, Result};
use serde_json::Value;
use std::fmt;
use std::fs;
use std::path::{Path, PathBuf};

/// CAPTCHA solver credentials used to preconfigure a bundled browser extension.
#[derive(Clone, PartialEq, Eq)]
pub struct CaptchaExtensionSeed {
    solver_id: String,
    api_key: String,
    base_url: String,
}

impl fmt::Debug for CaptchaExtensionSeed {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("CaptchaExtensionSeed")
            .field("solver_id", &self.solver_id)
            .field("api_key", &"<redacted>")
            .field("base_url", &self.base_url)
            .finish()
    }
}

impl CaptchaExtensionSeed {
    /// Creates a new built-in CAPTCHA extension seed.
    pub fn new(
        solver_id: impl Into<String>,
        api_key: impl Into<String>,
        base_url: impl Into<String>,
    ) -> Self {
        Self {
            solver_id: solver_id.into(),
            api_key: api_key.into(),
            base_url: base_url.into(),
        }
    }

    /// Returns the built-in solver id this seed targets.
    pub fn solver_id(&self) -> &str {
        &self.solver_id
    }

    /// Returns the decrypted API key for the extension.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// Returns the configured solver API base URL.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }
}

/// Returns the extension directory to load after applying static seed data.
pub fn stage_builtin_captcha_extension(
    source_dir: &Path,
    stage_root: &Path,
    seed: &CaptchaExtensionSeed,
) -> Result<PathBuf> {
    if seed.solver_id() != "nopecha" {
        return Ok(source_dir.to_path_buf());
    }
    let staged_dir = stage_root.join(seed.solver_id());
    reset_staged_dir(source_dir, &staged_dir)?;
    patch_nopecha_manifest(&staged_dir.join("manifest.json"), seed)?;
    flip_nopecha_force_base_api(&staged_dir.join("background.js"))?;
    Ok(staged_dir)
}

fn reset_staged_dir(source_dir: &Path, staged_dir: &Path) -> Result<()> {
    if staged_dir.exists() {
        fs::remove_dir_all(staged_dir).with_context(|| {
            format!("reset staged extension directory {}", staged_dir.display())
        })?;
    }
    copy_dir_all(source_dir, staged_dir)
}

fn copy_dir_all(source_dir: &Path, target_dir: &Path) -> Result<()> {
    fs::create_dir_all(target_dir)
        .with_context(|| format!("create extension stage {}", target_dir.display()))?;
    for entry in fs::read_dir(source_dir)
        .with_context(|| format!("read extension source {}", source_dir.display()))?
    {
        let entry = entry.context("read extension source entry")?;
        let source_path = entry.path();
        let target_path = target_dir.join(entry.file_name());
        if entry
            .file_type()
            .with_context(|| format!("read file type for {}", source_path.display()))?
            .is_dir()
        {
            copy_dir_all(&source_path, &target_path)?;
        } else {
            fs::copy(&source_path, &target_path).with_context(|| {
                format!(
                    "copy extension file {} to {}",
                    source_path.display(),
                    target_path.display()
                )
            })?;
        }
    }
    Ok(())
}

fn patch_nopecha_manifest(manifest_path: &Path, seed: &CaptchaExtensionSeed) -> Result<()> {
    let contents = fs::read_to_string(manifest_path)
        .with_context(|| format!("read NopeCHA manifest {}", manifest_path.display()))?;
    let mut manifest: Value =
        serde_json::from_str(&contents).context("parse NopeCHA manifest JSON")?;
    let Some(nopecha) = manifest.get_mut("nopecha").and_then(Value::as_object_mut) else {
        bail!("NopeCHA automation manifest is missing the `nopecha` object");
    };
    nopecha.insert("enabled".to_string(), Value::Bool(true));
    nopecha.insert("key".to_string(), Value::String(seed.api_key().to_string()));
    nopecha.insert(
        "_base_api".to_string(),
        Value::String(seed.base_url().to_string()),
    );
    let updated =
        serde_json::to_string_pretty(&manifest).context("serialize NopeCHA staged manifest")?;
    fs::write(manifest_path, updated)
        .with_context(|| format!("write NopeCHA staged manifest {}", manifest_path.display()))
}

/// The pinned NopeCHA `chromium_automation` build hard-codes `forceBaseApi: true`,
/// whose config merge overrides the manifest's `_base_api` host back to
/// `api.nopecha.com` — defeating the staged `_base_api`. Flip that single obfuscated
/// literal to `false` so the build honors `_base_api`. The anchor is unique to the
/// pinned bundle (catalog sha256 `4871e1c6…`): `i(608)+i(609)` decodes to the property
/// key `forceBaseApi` and `!t[0]` (shared constant `t[0] == 0`) is its `true` value;
/// `!t[1]` (`t[1] == 1`) is `false`. If upstream re-bundles NopeCHA the obfuscation
/// reshuffles and the anchor stops matching, in which case the bundle is left as-is
/// (host stays api.nopecha.com) and a warning is logged rather than failing the
/// browser launch.
const NOPECHA_FORCE_BASE_API_ANCHOR: &str = "i(608)+i(609)]:!t[0]";
const NOPECHA_FORCE_BASE_API_FLIPPED: &str = "i(608)+i(609)]:!t[1]";

fn flip_nopecha_force_base_api(background_js: &Path) -> Result<()> {
    if !background_js.exists() {
        return Ok(());
    }
    let content = fs::read_to_string(background_js).with_context(|| {
        format!(
            "read staged NopeCHA background.js {}",
            background_js.display()
        )
    })?;
    let count = content.matches(NOPECHA_FORCE_BASE_API_ANCHOR).count();
    if count != 1 {
        eprintln!(
            "puffer: NopeCHA forceBaseApi anchor matched {count} time(s) (expected 1) in {}; \
             leaving the host override in place — re-derive the anchor for this bundle",
            background_js.display()
        );
        return Ok(());
    }
    let patched = content.replace(
        NOPECHA_FORCE_BASE_API_ANCHOR,
        NOPECHA_FORCE_BASE_API_FLIPPED,
    );
    fs::write(background_js, patched).with_context(|| {
        format!(
            "write patched NopeCHA background.js {}",
            background_js.display()
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use tempfile::tempdir;

    #[test]
    fn stages_nopecha_manifest_with_static_key_config() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(source.join("nested")).unwrap();
        fs::write(source.join("nested/file.js"), "console.log('ok');").unwrap();
        fs::write(
            source.join("manifest.json"),
            serde_json::to_string_pretty(&json!({
                "name": "NopeCHA: CAPTCHA Solver",
                "manifest_version": 3,
                "key": "stable-extension-id-key",
                "nopecha": {
                    "enabled": false,
                    "key": "",
                    "_base_api": "",
                    "recaptcha_auto_solve": true
                }
            }))
            .unwrap(),
        )
        .unwrap();
        let seed = CaptchaExtensionSeed::new("nopecha", "paid-key", "https://api.example.test");

        let staged =
            stage_builtin_captcha_extension(&source, &dir.path().join("stage"), &seed).unwrap();

        assert_eq!(staged, dir.path().join("stage/nopecha"));
        assert_eq!(
            fs::read_to_string(staged.join("nested/file.js")).unwrap(),
            "console.log('ok');"
        );
        let manifest: Value =
            serde_json::from_str(&fs::read_to_string(staged.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(manifest["key"], "stable-extension-id-key");
        assert_eq!(manifest["nopecha"]["enabled"], true);
        assert_eq!(manifest["nopecha"]["key"], "paid-key");
        assert_eq!(manifest["nopecha"]["_base_api"], "https://api.example.test");
        let source_manifest: Value =
            serde_json::from_str(&fs::read_to_string(source.join("manifest.json")).unwrap())
                .unwrap();
        assert_eq!(source_manifest["nopecha"]["key"], "");
    }

    #[test]
    fn leaves_runtime_seeded_solvers_at_source_dir() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(&source).unwrap();
        let seed = CaptchaExtensionSeed::new("2captcha", "key", "https://2captcha.test");

        let resolved =
            stage_builtin_captcha_extension(&source, &dir.path().join("stage"), &seed).unwrap();

        assert_eq!(resolved, source);
        assert!(!dir.path().join("stage").exists());
    }

    #[test]
    fn flips_nopecha_force_base_api_in_staged_background() {
        let dir = tempdir().unwrap();
        let source = dir.path().join("source");
        fs::create_dir_all(&source).unwrap();
        fs::write(
            source.join("manifest.json"),
            serde_json::to_string_pretty(&json!({
                "name": "NopeCHA: CAPTCHA Solver",
                "manifest_version": 3,
                "nopecha": { "enabled": false, "key": "", "_base_api": "" }
            }))
            .unwrap(),
        )
        .unwrap();
        fs::write(
            source.join("background.js"),
            "x=ot({...a,...s},{[i(607)+t[140]]:_1,[i(608)+i(609)]:!t[0]});y=!t[0]",
        )
        .unwrap();
        let seed = CaptchaExtensionSeed::new("nopecha", "k", "https://api.example.test");

        let staged =
            stage_builtin_captcha_extension(&source, &dir.path().join("stage"), &seed).unwrap();

        let bg = fs::read_to_string(staged.join("background.js")).unwrap();
        // The forceBaseApi literal flips to false; the unrelated trailing !t[0] stays.
        assert!(bg.contains("[i(608)+i(609)]:!t[1]"));
        assert!(!bg.contains("[i(608)+i(609)]:!t[0]"));
        assert!(bg.ends_with("y=!t[0]"));
        // The source bundle is left pristine.
        assert!(fs::read_to_string(source.join("background.js"))
            .unwrap()
            .contains("[i(608)+i(609)]:!t[0]"));
    }
}
