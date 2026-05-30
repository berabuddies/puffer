//! U-card backend credentials for the `momo-card` reveal bin.
//!
//! Writes the ucard-backend base URL + the Auth-Station JWT into
//! `~/.ucard/.creds` (0600) so the `momo-card` bin can authenticate itself
//! (JWT -> sessionToken -> /card/details) without the credential ever passing
//! through the agent's context. The JWT is the same `puffer.authToken` momo
//! already holds in the frontend.
//!
//! CARD DATA (PAN/CVV) NEVER GOES HERE — only the base URL + JWT. The PAN/CVV
//! live exclusively inside the `momo-card` process at reveal time.

use std::io::Write;
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;

/// Render the creds body: two `KEY=VALUE` lines + trailing newline.
pub fn render_creds(base_url: &str, jwt: &str) -> String {
    format!("UCARD_BASE_URL={base_url}\nUCARD_JWT={jwt}\n")
}

/// Write `<dir>/.creds` with mode 0600. Creates `dir` if missing; truncates
/// any existing file.
pub fn write_creds_file(dir: &Path, base_url: &str, jwt: &str) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(".creds");
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(render_creds(base_url, jwt).as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn renders_two_lines_with_trailing_newline() {
        assert_eq!(
            render_creds("https://api.example.com", "jwt-abc"),
            "UCARD_BASE_URL=https://api.example.com\nUCARD_JWT=jwt-abc\n"
        );
    }

    #[test]
    fn writes_file_with_0600_and_both_vars() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".ucard");
        write_creds_file(&dir, "https://api.example.com", "jwt-xyz").unwrap();
        let path = dir.join(".creds");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("UCARD_BASE_URL=https://api.example.com"));
        assert!(body.contains("UCARD_JWT=jwt-xyz"));
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn rewrites_truncate_old_content() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join(".ucard");
        write_creds_file(&dir, "https://old", "jwt-old").unwrap();
        write_creds_file(&dir, "https://new", "jwt-new").unwrap();
        let body = std::fs::read_to_string(dir.join(".creds")).unwrap();
        assert!(body.contains("jwt-new"));
        assert!(!body.contains("jwt-old"));
        assert!(!body.contains("https://old"));
    }
}
