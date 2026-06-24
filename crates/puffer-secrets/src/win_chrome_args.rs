//! Pure argument validation / escaping for the Windows Chrome v20 import's
//! self-elevation (`win_chrome_import`). Kept in its own platform-independent
//! module so this security-critical logic — which decides what reaches the
//! `powershell` / `schtasks` command lines and the SYSTEM-stage filesystem paths —
//! can be unit-tested on any host (the rest of `win_chrome_import` is Windows-only
//! and needs a real Windows runtime). Compiled on Windows (its only caller) and in
//! test builds everywhere.

use anyhow::{bail, Context, Result};

/// Rejects a value that cannot be SAFELY embedded in the elevated command lines
/// (the `schtasks /tr` action and the PowerShell `-ArgumentList`) or in the
/// SYSTEM-stage filesystem paths. Windows account names and paths cannot contain
/// `"`, and we additionally forbid control chars / newlines that no quoting can
/// neutralise and a trailing backslash that would escape our literal closing `\"`.
/// Everything else (spaces, `'`, `&`, `(`, `)`, `%`, ...) is rendered inert by
/// double-quoting (schtasks, where `"` is the only breakout) and single-quote
/// doubling (PowerShell, where `'` is the only breakout), so we do NOT reject those
/// — staying compatible with real usernames and paths. Validated at EVERY stage
/// (user / elevated / system), because the hidden `__win-chrome-import` subcommand
/// is directly invokable with arbitrary args.
pub(crate) fn validate_elevation_arg(kind: &str, value: &str) -> Result<()> {
    if value.is_empty() || value.len() > 32_768 {
        bail!("refusing to elevate: {kind} is empty or too long");
    }
    // Reject the ASCII double quote AND the Unicode "smart" quote codepoints that
    // PowerShell's tokenizer treats as equivalent to ASCII quotes (U+2018..U+201B
    // single, U+201C..U+201E double): otherwise a curly single quote could close
    // our single-quoted PowerShell argument and inject — the ASCII apostrophe is
    // neutralised by ps_single_quote, but these are not. Also reject control chars.
    const QUOTE_CLASS: &[char] = &[
        '"', '\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}', '\u{201C}', '\u{201D}', '\u{201E}',
    ];
    if value.chars().any(|c| QUOTE_CLASS.contains(&c) || c.is_control()) {
        bail!("refusing to elevate: {kind} contains an unsupported character");
    }
    // A trailing backslash escapes the literal closing `\"` we wrap the value in,
    // letting it swallow the next token when CommandLineToArgvW re-parses the line
    // (the classic Windows trailing-backslash break). No legitimate account name
    // survives this; vault directories are normalised by normalize_vault_dir first.
    if value.ends_with('\\') {
        bail!("refusing to elevate: {kind} must not end with a backslash");
    }
    Ok(())
}

/// Trims trailing path separators from a vault directory so a user-supplied value
/// like `D:\puffer\` is accepted (it is semantically identical) instead of tripping
/// the trailing-backslash rejection. A bare drive root (`X:\` / `X:/`) keeps one
/// separator so it stays drive-absolute rather than collapsing to a drive-relative
/// `X:`; a bare value is left unchanged.
pub(crate) fn normalize_vault_dir(vault: &str) -> String {
    let trimmed = vault.trim_end_matches(|c| c == '\\' || c == '/');
    let bytes = trimmed.as_bytes();
    if trimmed.is_empty() {
        vault.to_string()
    } else if bytes.len() == 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        // `C:\` / `C:///` -> `C:\` (don't turn a drive root into drive-relative `C:`)
        format!("{trimmed}\\")
    } else {
        trimmed.to_string()
    }
}

/// Validates the vault DIRECTORY, which (unlike the account name) legitimately
/// contains backslashes. The SYSTEM stage opens/creates the vault here and writes
/// the master key + encrypted store, so a crafted value must not redirect that
/// write off the local machine or up the tree: require a local drive-letter
/// absolute path (`X:\...`), reject UNC roots (`\\host\share`, `//host`), and reject
/// any `..` segment. Pass the value through [`normalize_vault_dir`] first.
pub(crate) fn validate_vault_dir(vault: &str) -> Result<()> {
    validate_elevation_arg("vault directory", vault)?;
    let bytes = vault.as_bytes();
    let is_unc = vault.starts_with("\\\\") || vault.starts_with("//");
    let is_drive_abs = bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && (bytes[2] == b'\\' || bytes[2] == b'/');
    if is_unc || !is_drive_abs {
        bail!("refusing to elevate: vault directory must be a local drive-absolute path");
    }
    if vault.split(|c| c == '\\' || c == '/').any(|seg| seg == "..") {
        bail!("refusing to elevate: vault directory must not contain a `..` segment");
    }
    Ok(())
}

/// Stricter validation for a Windows account name, which is ALSO interpolated into
/// the fixed `C:\Users\{user}\...` profile path the SYSTEM stage reads/writes.
/// Beyond [`validate_elevation_arg`] it rejects path separators, a drive marker,
/// and `.`/`..` segments so a crafted name cannot redirect the SYSTEM-stage file
/// access outside the user's profile. Real logon names contain none of these.
pub(crate) fn validate_account_name(user: &str) -> Result<()> {
    validate_elevation_arg("account name", user)?;
    if user.chars().any(|c| matches!(c, '\\' | '/' | ':')) || user == "." || user == ".." {
        bail!("refusing to elevate: account name contains a path separator");
    }
    Ok(())
}

/// Escapes a value for embedding inside a PowerShell single-quoted string: there
/// the only metacharacter is `'`, escaped by doubling it.
pub(crate) fn ps_single_quote(value: &str) -> String {
    value.replace('\'', "''")
}

/// Parses the cross-stage PID argument, rejecting a missing/invalid/zero value
/// rather than defaulting to 0. A 0 would make the result filename
/// (`..._0.txt`) and the transient task name (`PufferChromeImport_0`) fixed and
/// predictable for direct invocations of the hidden subcommand, and is never a
/// real process id — so fail closed.
pub(crate) fn parse_required_pid(arg: Option<&String>) -> Result<u32> {
    arg.and_then(|s| s.parse::<u32>().ok())
        .filter(|&pid| pid != 0)
        .context("missing or invalid PID argument for the import helper")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ---- normalize_vault_dir (PR-review #1: drive root must not collapse) -------

    #[test]
    fn normalize_trims_trailing_separators_but_keeps_drive_root() {
        // A trailing separator on a real dir is stripped (so it isn't rejected).
        assert_eq!(normalize_vault_dir("D:\\puffer\\"), "D:\\puffer");
        assert_eq!(normalize_vault_dir("D:\\puffer"), "D:\\puffer");
        assert_eq!(normalize_vault_dir("C:/Users/me/.puffer//"), "C:/Users/me/.puffer");
        // PR-review #1: a bare drive root keeps one separator (stays drive-absolute),
        // it must NOT collapse to the drive-relative `C:`.
        assert_eq!(normalize_vault_dir("C:\\"), "C:\\");
        assert_eq!(normalize_vault_dir("C:/"), "C:\\");
        assert_eq!(normalize_vault_dir("Z:\\\\\\"), "Z:\\");
        // Defense in depth: a *bare drive root* is still rejected downstream — its
        // trailing separator would break the `/tr` quoting (validate_elevation_arg
        // rejects a trailing backslash), and it is never a real vault location. The
        // point of the normalize fix is only that it no longer SILENTLY rewrites
        // `C:\` to the drive-relative `C:`; the rejection is intentional.
        assert!(validate_vault_dir(&normalize_vault_dir("C:\\")).is_err());
        // An all-separators value is left unchanged (won't pass validation anyway).
        assert_eq!(normalize_vault_dir("\\\\"), "\\\\");
    }

    // ---- validate_vault_dir (round-2 traversal/UNC hardening) -------------------

    #[test]
    fn vault_dir_accepts_local_drive_absolute_paths() {
        for ok in [
            "C:\\Users\\me\\.puffer",
            "D:\\puffer",
            "C:/Users/me/.puffer",
            "C:\\Program Files (x86)\\x", // parens/spaces are fine
            "C:\\Users\\O'Brien\\.puffer", // apostrophe is fine (PS-escaped later)
        ] {
            assert!(validate_vault_dir(ok).is_ok(), "should accept: {ok}");
        }
    }

    #[test]
    fn vault_dir_rejects_unc_traversal_and_relative() {
        for bad in [
            "\\\\attacker\\share",      // UNC -> off-machine redirect
            "//attacker/share",          // forward-slash UNC
            "C:\\Users\\..\\..\\Windows", // .. traversal
            "C:\\a\\..\\b",
            "puffer",                    // relative
            "C:puffer",                  // drive-relative (no separator)
            "\\Users\\me",               // rooted-but-no-drive
        ] {
            assert!(validate_vault_dir(bad).is_err(), "should reject: {bad}");
        }
    }

    // ---- validate_elevation_arg (round-1/2 injection: quotes/control/backslash) -

    #[test]
    fn elevation_arg_rejects_ascii_and_unicode_quotes() {
        // ASCII double quote.
        assert!(validate_elevation_arg("v", "a\"b").is_err());
        // Round-2 finding: every Unicode "smart" quote PowerShell treats as a quote.
        for q in ['\u{2018}', '\u{2019}', '\u{201A}', '\u{201B}', '\u{201C}', '\u{201D}', '\u{201E}'] {
            let payload = format!("x{q};calc;#");
            assert!(
                validate_elevation_arg("v", &payload).is_err(),
                "must reject smart quote U+{:04X}",
                q as u32
            );
        }
    }

    #[test]
    fn elevation_arg_rejects_control_chars_and_trailing_backslash() {
        assert!(validate_elevation_arg("v", "a\nb").is_err()); // newline / control
        assert!(validate_elevation_arg("v", "a\u{0}b").is_err()); // NUL
        assert!(validate_elevation_arg("v", "C:\\dir\\").is_err()); // trailing backslash break
        assert!(validate_elevation_arg("v", "").is_err()); // empty
    }

    #[test]
    fn elevation_arg_keeps_benign_characters() {
        // These are inert inside our quoting and must NOT be over-rejected.
        for ok in ["O'Brien", "a & b", "x (y)", "100% sure", "a`b", "a$b", "name.with.dots"] {
            assert!(validate_elevation_arg("v", ok).is_ok(), "should accept: {ok}");
        }
    }

    // ---- validate_account_name (round-1 path-separator/traversal hardening) -----

    #[test]
    fn account_name_accepts_real_logon_names_rejects_traversal() {
        for ok in ["Administrator", "john.doe", "O'Brien", "My Name"] {
            assert!(validate_account_name(ok).is_ok(), "should accept: {ok}");
        }
        for bad in ["..", ".", "a\\b", "a/b", "DOMAIN\\user", "C:"] {
            assert!(validate_account_name(bad).is_err(), "should reject: {bad}");
        }
        // It inherits the quote/control/backslash rejections too.
        assert!(validate_account_name("a\u{2019}b").is_err());
    }

    // ---- ps_single_quote (round-2 PowerShell escaping) -------------------------

    #[test]
    fn ps_single_quote_doubles_apostrophes() {
        assert_eq!(ps_single_quote("O'Brien"), "O''Brien");
        assert_eq!(ps_single_quote("a'b'c"), "a''b''c");
        assert_eq!(ps_single_quote("clean"), "clean");
    }

    // ---- parse_required_pid (PR-review #3/#4: no predictable pid=0 default) -----

    #[test]
    fn parse_required_pid_accepts_real_pids() {
        assert_eq!(parse_required_pid(Some(&"1234".to_string())).unwrap(), 1234);
        assert_eq!(parse_required_pid(Some(&"1".to_string())).unwrap(), 1);
    }

    #[test]
    fn parse_required_pid_rejects_missing_invalid_or_zero() {
        // PR-review #3/#4: missing/invalid/0 must fail closed, NOT default to 0
        // (which would make ..._0.txt / PufferChromeImport_0 predictable).
        assert!(parse_required_pid(None).is_err());
        assert!(parse_required_pid(Some(&"0".to_string())).is_err());
        assert!(parse_required_pid(Some(&"".to_string())).is_err());
        assert!(parse_required_pid(Some(&"notanumber".to_string())).is_err());
        assert!(parse_required_pid(Some(&"12 & calc".to_string())).is_err());
        assert!(parse_required_pid(Some(&"-5".to_string())).is_err());
    }
}
