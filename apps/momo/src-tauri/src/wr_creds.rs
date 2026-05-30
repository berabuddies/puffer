//! WorldRouter service credentials.
//!
//! Writes the user's WorldRouter API key + control-api base URL into
//! `~/.wr/.creds` (0600) so WorldRouter-backed skills (e.g. `book-by-phone`)
//! can `source` it from the agent's bash without an env-injection dance. The
//! file is written by momo on login; the key is the same cached
//! `sk-worldrouter-…` key momo already uses for inference.
//!
//! Card data (PAN/CVV) NEVER goes here — that path is intentionally separate
//! and far more restricted (see docs/superpowers/plans Part B).

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Render the creds file body: two `KEY=VALUE` lines + trailing newline so
/// `source ~/.wr/.creds` exports both variables.
pub fn render_creds(api_key: &str, base_url: &str) -> String {
    format!("WORLDROUTER_API_KEY={api_key}\nWORLDROUTER_BASE_URL={base_url}\n")
}

/// Write `<dir>/.creds` with mode 0600 (owner read/write only — it holds a
/// secret). Creates `dir` if missing; truncates any existing file.
pub fn write_creds_file(dir: &Path, api_key: &str, base_url: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(".creds");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(render_creds(api_key, base_url).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn renders_two_lines_with_trailing_newline() {
        assert_eq!(
            render_creds("sk-worldrouter-abc", "https://control-api.worldrouter.ai"),
            "WORLDROUTER_API_KEY=sk-worldrouter-abc\nWORLDROUTER_BASE_URL=https://control-api.worldrouter.ai\n"
        );
    }

    #[test]
    fn writes_file_with_0600_and_both_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let wr = tmp.path().join(".wr");
        write_creds_file(&wr, "sk-x", "https://control-api-test-0f9c17.worldrouter.ai").unwrap();
        let path = wr.join(".creds");

        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("WORLDROUTER_API_KEY=sk-x"));
        assert!(body.contains("WORLDROUTER_BASE_URL=https://control-api-test-0f9c17.worldrouter.ai"));

        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn rewrites_truncate_old_content() {
        let tmp = tempfile::tempdir().unwrap();
        let wr = tmp.path().join(".wr");
        write_creds_file(&wr, "sk-old", "https://old").unwrap();
        write_creds_file(&wr, "sk-new", "https://new").unwrap();
        let body = std::fs::read_to_string(wr.join(".creds")).unwrap();
        assert!(body.contains("sk-new"));
        assert!(!body.contains("sk-old"));
        assert!(!body.contains("https://old"));
    }
}
