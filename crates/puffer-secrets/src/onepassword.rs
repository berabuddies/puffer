//! 1Password integration — two ways a 1Password secret reaches an agent.
//!
//! 1. **Import (eager, unified with browsers):** `sync_onepassword_references`
//!    lists Login items and resolves each `op://vault/item/field` reference to
//!    its live value via the 1Password CLI (`op read`), then stores the
//!    plaintext in Puffer's vault (encrypted at rest, AES-256-GCM) just like a
//!    browser-imported password. After import the agent no longer needs `op` or
//!    a token at request time. Trade-off: a rotated 1Password password goes
//!    stale until the next sync.
//! 2. **Resolve-on-demand:** a secret whose stored value is still an `op://`
//!    reference (e.g. one added manually via the "Add secret" form) is resolved
//!    at request time, so its plaintext never persists. `is_op_reference` /
//!    `resolve_op_reference` back this path (see runtime `request_secret`).
//!
//! Authorization (no token hunting): `op` authorizes itself one of two ways.
//! Preferred for a desktop user is the **1Password app's CLI integration** — the
//! user enables it once (1Password app -> Settings -> Developer -> "Integrate
//! with 1Password CLI"), after which every `op` call is approved with Touch ID
//! at sync time; nothing to find or paste. For headless/daemon use a
//! service-account token (`OP_SERVICE_ACCOUNT_TOKEN`) works instead. Either way
//! `op` reads its own credentials, so no token handling lives here;
//! `ensure_op_authorized` only checks that one of the two is set up and, if not,
//! points the user at the one-click path.

use anyhow::{bail, Context, Result};
use std::io::Read;
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const OP_REFERENCE_PREFIX: &str = "op://";

/// Hard cap on a single `op` data call. The desktop app-integration call is known
/// to hang indefinitely (ignoring internal timeouts) if the app locks mid-call
/// (onepassword-sdk-go#266), so every list/read goes through this watchdog.
const OP_CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// Runs an `op` command with a hard timeout. stdout/stderr are drained in reader
/// threads (so a large output can't deadlock the child) and the process is killed
/// on timeout — preventing the app-lock hang documented in onepassword-sdk-go#266.
fn run_op(op_bin: &str, args: &[&str], timeout: Duration) -> Result<Output> {
    let mut child = Command::new(op_bin)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("spawn `{op_bin}`"))?;
    let mut out = child.stdout.take().expect("piped stdout");
    let mut err = child.stderr.take().expect("piped stderr");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = out.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = err.read_to_end(&mut buf);
        buf
    });
    let start = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = out_reader.join();
            let _ = err_reader.join();
            bail!(
                "1Password CLI timed out after {}s — the desktop app may be locked; unlock it and try again",
                timeout.as_secs()
            );
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    let stdout = out_reader.join().unwrap_or_default();
    let stderr = err_reader.join().unwrap_or_default();
    Ok(Output { status, stdout, stderr })
}

/// Returns whether a stored secret value is a 1Password secret reference.
pub fn is_op_reference(value: &str) -> bool {
    value.trim_start().starts_with(OP_REFERENCE_PREFIX)
}

/// Reports whether the 1Password CLI (`op`) is available on this machine.
pub fn op_cli_available() -> bool {
    Command::new("op")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

/// Reports whether `op` has a usable credential source: either a service-account
/// token in the environment, or at least one account connected through the
/// 1Password app's CLI integration. With app integration, each request is
/// authorized with Touch ID at the moment of sync — so the user never has to
/// hunt for or paste a token.
pub fn op_authorized() -> bool {
    std::env::var_os("OP_SERVICE_ACCOUNT_TOKEN").is_some() || op_has_connected_account("op")
}

/// Returns `Ok(())` when 1Password access is set up, otherwise an actionable
/// error that guides the user to one-click (biometric) authorization rather than
/// to a service-account token.
pub fn ensure_op_authorized() -> Result<()> {
    if op_authorized() {
        return Ok(());
    }
    bail!(
        "1Password is not connected. To sync directly, open the 1Password app, sign in, \
then Settings -> Developer -> turn on \"Integrate with 1Password CLI\" (after that a sync \
just asks for Touch ID / Windows Hello — no token). Or skip the CLI and import a \
1Password export instead: in the app File -> Export -> 1PUX, then use \"Import 1Password \
export (.1pux)\". (Headless alternative: set OP_SERVICE_ACCOUNT_TOKEN.) \
See https://developer.1password.com/docs/cli/app-integration/"
    )
}

/// Whether the 1Password desktop app is installed (so the app's CLI integration
/// is possible). Used to decide whether auto-installing the `op` CLI is worthwhile.
pub fn onepassword_app_installed() -> bool {
    #[cfg(target_os = "macos")]
    {
        std::path::Path::new("/Applications/1Password.app").exists()
            || dirs::home_dir()
                .map(|home| home.join("Applications/1Password.app").exists())
                .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|var| std::env::var_os(var))
            .any(|base| std::path::Path::new(&base).join("1Password\\app").is_dir())
    }
    #[cfg(target_os = "linux")]
    {
        std::path::Path::new("/opt/1Password/1password").exists()
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

/// Best-effort auto-install of the `op` CLI via the platform package manager, used
/// when the 1Password app is present but the CLI is not. Returns Ok(()) if the
/// install command ran successfully; otherwise an error pointing at the manual
/// install. Whether `op` is immediately on the running process's PATH is left to
/// the caller (a freshly-installed `op` may need a Puffer restart to be picked up).
fn install_op_cli() -> Result<()> {
    const DOCS: &str = "https://developer.1password.com/docs/cli/get-started/";

    #[cfg(target_os = "macos")]
    let mut cmd = {
        let mut c = Command::new("brew");
        // Run non-interactively (no tty): skip the slow auto-update and don't
        // fail on the user's pre-existing untrusted third-party taps.
        c.args(["install", "--cask", "1password-cli"])
            .env("NONINTERACTIVE", "1")
            .env("HOMEBREW_NO_AUTO_UPDATE", "1")
            .env("HOMEBREW_NO_INSTALL_UPGRADE", "1")
            .env("HOMEBREW_NO_REQUIRE_TAP_TRUST", "1");
        c
    };
    #[cfg(target_os = "windows")]
    let mut cmd = {
        let mut c = Command::new("winget");
        c.args([
            "install", "--id", "AgileBits.1Password.CLI", "-e", "--silent",
            "--accept-source-agreements", "--accept-package-agreements",
        ]);
        c
    };
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        bail!("No auto-installer for this platform. Install the 1Password CLI manually: {DOCS}");
    }

    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        // Capture stdio so the installer never inherits the daemon's handshake
        // pipe — writing progress to it can break with EPIPE and fail the
        // install. A null stdin avoids any prompt-wait.
        let output = cmd
            .stdin(Stdio::null())
            .output()
            .context("spawn the 1Password CLI installer")?;
        if output.status.success() {
            return Ok(());
        }
        let stderr = String::from_utf8_lossy(&output.stderr);
        let detail = stderr.trim().lines().last().unwrap_or("install failed");
        bail!("Couldn't auto-install the 1Password CLI ({detail}). Install it manually: {DOCS}");
    }
}

/// Best-effort launch of the 1Password desktop app so the user can sign in or
/// unlock it. Returns whether a launch command was successfully dispatched
/// (false e.g. on a headless box with no app installed).
pub fn launch_onepassword_app() -> bool {
    #[cfg(target_os = "macos")]
    {
        Command::new("open")
            .args(["-a", "1Password"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "windows")]
    {
        // Only launch via the `onepassword://` protocol if the app is actually
        // installed — otherwise Windows pops a confusing "you'll need a new app
        // to open this link" dialog. When absent, report false so the caller
        // surfaces the actionable guidance (token / install) instead.
        if !onepassword_app_installed() {
            return false;
        }
        Command::new("cmd")
            .args(["/C", "start", "", "onepassword://"])
            .status()
            .map(|status| status.success())
            .unwrap_or(false)
    }
    #[cfg(target_os = "linux")]
    {
        Command::new("1password")
            .spawn()
            .map(|_| true)
            .unwrap_or(false)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        false
    }
}

/// Detect-first 1Password access: if `op` is already authorized (a service
/// account token, or a signed-in account via the app's CLI integration), proceed
/// to sync. Otherwise open the app best-effort (non-blocking) so the user can
/// sign in / enable the integration, and return actionable guidance immediately —
/// no long blocking wait. The caller/UI surfaces the guidance and can offer the
/// `.1pux` export-file import as a no-CLI alternative.
pub fn connect_onepassword() -> Result<()> {
    if op_authorized() {
        return Ok(());
    }
    // The 1Password app is here but the `op` CLI isn't: auto-install the CLI, then
    // point the user at the app's CLI-integration toggle (which only the user can
    // flip) and ask them to retry — after that a sync just needs Touch ID /
    // Windows Hello. (If `op` is present but unauthorized, fall through to the
    // generic guidance below.)
    if !op_cli_available() && onepassword_app_installed() {
        install_op_cli()?;
        bail!(
            "Installed the 1Password CLI. Now open the 1Password app -> Settings -> \
Developer -> turn on \"Integrate with 1Password CLI\", then run the sync again \
(restart Puffer first if it still says the CLI isn't found)."
        );
    }
    let _ = launch_onepassword_app();
    ensure_op_authorized()
}

/// Whether `op` lists at least one connected account. `op account list` reads
/// local config and does NOT require authentication, so this is a cheap probe of
/// the app-integration (biometric) path being set up.
fn op_has_connected_account(op_bin: &str) -> bool {
    let Ok(output) = Command::new(op_bin)
        .args(["account", "list", "--format", "json"])
        .output()
    else {
        return false;
    };
    if !output.status.success() {
        return false;
    }
    serde_json::from_slice::<serde_json::Value>(&output.stdout)
        .ok()
        .and_then(|value| value.as_array().map(|accounts| !accounts.is_empty()))
        .unwrap_or(false)
}

/// Resolves a `op://vault/item/field` reference to its live value via `op read`.
pub fn resolve_op_reference(reference: &str) -> Result<String> {
    resolve_with("op", reference)
}

/// Resolution core parameterized by the CLI binary, for testing with a fake `op`.
fn resolve_with(op_bin: &str, reference: &str) -> Result<String> {
    let reference = reference.trim();
    if !is_op_reference(reference) {
        bail!("`{reference}` is not a 1Password secret reference (op://...)");
    }
    let output = run_op(op_bin, &["read", "--no-newline", reference], OP_CALL_TIMEOUT)
        .context("run 1Password CLI `op read` (is `op` installed and OP_SERVICE_ACCOUNT_TOKEN set?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`op read {reference}` failed: {}", stderr.trim());
    }
    let value = String::from_utf8(output.stdout).context("1Password value is not UTF-8")?;
    // `--no-newline` should prevent a trailing newline, but strip defensively.
    let value = value
        .strip_suffix('\n')
        .map(str::to_string)
        .unwrap_or(value);
    if value.is_empty() {
        bail!("1Password reference `{reference}` resolved to an empty value");
    }
    Ok(value)
}

/// One 1Password login item the listing surfaced (ids only, no values yet).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpItemRef {
    /// Stable item id.
    pub id: String,
    /// Id of the vault the item lives in.
    pub vault_id: String,
    /// Display label (the item title).
    pub label: String,
    /// Optional non-secret origin URL.
    pub origin: Option<String>,
}

/// One 1Password login with its values already revealed, ready to store.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedLogin {
    pub label: String,
    pub username: Option<String>,
    pub password: String,
    pub origin: Option<String>,
}

/// Batch-imports Login items WITH their values: lists every Login item the
/// current auth can see, then fetches each in one structured
/// `op item get --reveal --format json` call (username + password + url
/// together, safely JSON-encoded by `op`). This halves the subprocess count
/// versus two `op read`s per item and, under the desktop-app integration, the
/// whole batch is covered by a single biometric authorization.
///
/// Returns the resolved logins plus a per-item error list: a single item that
/// fails to fetch (e.g. a flaky/locked item that trips the watchdog) is skipped
/// and reported, NOT fatal — so one bad item can't discard the whole import.
/// A failure to even *list* the items still aborts (nothing to import).
pub fn import_resolved_logins() -> Result<(Vec<ResolvedLogin>, Vec<String>)> {
    import_resolved_with("op")
}

fn import_resolved_with(op_bin: &str) -> Result<(Vec<ResolvedLogin>, Vec<String>)> {
    let refs = list_login_refs(op_bin)?;
    let mut out = Vec::with_capacity(refs.len());
    let mut errors = Vec::new();
    for item in refs {
        match get_login_item(op_bin, &item) {
            Ok(login) => out.push(login),
            Err(error) => errors.push(format!("{}: {error}", item.label)),
        }
    }
    Ok((out, errors))
}

/// Lists every Login + Password item ref (no values) the current auth can see.
/// Both categories are included so the `op` CLI path covers the same items as
/// the `.1pux` export path (which imports Login 001 + Password 005).
fn list_login_refs(op_bin: &str) -> Result<Vec<OpItemRef>> {
    let output = run_op(
        op_bin,
        &["item", "list", "--categories", "Login,Password", "--format", "json"],
        OP_CALL_TIMEOUT,
    )
    .context("run 1Password CLI `op item list` (is `op` installed and OP_SERVICE_ACCOUNT_TOKEN set?)")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`op item list` failed: {}", stderr.trim());
    }
    parse_login_list(&output.stdout)
}

/// Fetches one login item with values via `op item get --reveal --format json`.
fn get_login_item(op_bin: &str, item: &OpItemRef) -> Result<ResolvedLogin> {
    let output = run_op(
        op_bin,
        &[
            "item",
            "get",
            &item.id,
            "--vault",
            &item.vault_id,
            "--reveal",
            "--format",
            "json",
        ],
        OP_CALL_TIMEOUT,
    )
    .context("run 1Password CLI `op item get`")?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("`op item get {}` failed: {}", item.id, stderr.trim());
    }
    parse_login_item(&output.stdout, item)
}

/// Extracts username + password (+ title/url fallbacks) from `op item get` JSON.
fn parse_login_item(stdout: &[u8], item: &OpItemRef) -> Result<ResolvedLogin> {
    let value: serde_json::Value =
        serde_json::from_slice(stdout).context("parse `op item get` JSON")?;
    let field_by_purpose = |purpose: &str| -> Option<String> {
        value
            .get("fields")
            .and_then(|fields| fields.as_array())
            .and_then(|fields| {
                fields
                    .iter()
                    .find(|field| {
                        field
                            .get("purpose")
                            .and_then(|value| value.as_str())
                            .map(|value| value.eq_ignore_ascii_case(purpose))
                            .unwrap_or(false)
                    })
                    .and_then(|field| field.get("value"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
    };
    // Login items expose the password via purpose=PASSWORD; standalone Password
    // (category 005) items have no purpose, so fall back to the first non-empty
    // concealed field (the item's password).
    let first_concealed = || -> Option<String> {
        value
            .get("fields")
            .and_then(|fields| fields.as_array())
            .and_then(|fields| {
                fields
                    .iter()
                    .find(|field| {
                        let concealed = field
                            .get("type")
                            .and_then(|value| value.as_str())
                            .map(|value| value.eq_ignore_ascii_case("CONCEALED"))
                            .unwrap_or(false);
                        let has_value = field
                            .get("value")
                            .and_then(|value| value.as_str())
                            .map(|value| !value.is_empty())
                            .unwrap_or(false);
                        concealed && has_value
                    })
                    .and_then(|field| field.get("value"))
                    .and_then(|value| value.as_str())
                    .map(str::to_string)
            })
    };
    let password = field_by_purpose("PASSWORD")
        .filter(|value| !value.is_empty())
        .or_else(first_concealed)
        .unwrap_or_default();
    let username = field_by_purpose("USERNAME").filter(|value| !value.is_empty());
    let label = value
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| item.label.clone());
    let origin = item.origin.clone().or_else(|| primary_url(&value));
    Ok(ResolvedLogin {
        label,
        username,
        password,
        origin,
    })
}

/// Parses `op item list --format json` output into login item refs (no values).
fn parse_login_list(stdout: &[u8]) -> Result<Vec<OpItemRef>> {
    let items: serde_json::Value =
        serde_json::from_slice(stdout).context("parse `op item list` JSON")?;
    let array = items
        .as_array()
        .context("`op item list` did not return a JSON array")?;
    let mut out = Vec::new();
    for item in array {
        let (Some(id), Some(vault_id)) = (
            item.get("id").and_then(|value| value.as_str()),
            item.get("vault")
                .and_then(|vault| vault.get("id"))
                .and_then(|value| value.as_str()),
        ) else {
            continue;
        };
        let label = item
            .get("title")
            .and_then(|value| value.as_str())
            .unwrap_or("1Password item")
            .to_string();
        out.push(OpItemRef {
            id: id.to_string(),
            vault_id: vault_id.to_string(),
            label,
            origin: primary_url(item),
        });
    }
    Ok(out)
}

/// Picks an item's primary URL (or the first one) from its `urls` array. Handles
/// both `op item list` (`urls[].href`) and `op item get` (`urls[].href`).
fn primary_url(item: &serde_json::Value) -> Option<String> {
    item.get("urls")
        .and_then(|urls| urls.as_array())
        .and_then(|urls| {
            urls.iter()
                .find(|url| {
                    url.get("primary")
                        .and_then(|primary| primary.as_bool())
                        .unwrap_or(false)
                })
                .or_else(|| urls.first())
        })
        .and_then(|url| url.get("href"))
        .and_then(|href| href.as_str())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_op_references() {
        assert!(is_op_reference("op://Private/GitHub/credential"));
        assert!(is_op_reference("  op://vault/item/field"));
        assert!(!is_op_reference("ghp_realtoken"));
        assert!(!is_op_reference("https://example.com"));
    }

    #[test]
    fn rejects_non_reference() {
        assert!(resolve_with("op", "not-a-ref").is_err());
    }

    #[test]
    fn parses_login_list_into_refs() {
        let json = br#"[
            {"id":"abc123","title":"GitHub","vault":{"id":"vlt1","name":"Private"},
             "category":"LOGIN","urls":[{"primary":true,"href":"https://github.com"}]},
            {"id":"def456","title":"No URL","vault":{"id":"vlt1","name":"Private"},"category":"LOGIN"}
        ]"#;
        let refs = parse_login_list(json).unwrap();
        assert_eq!(refs.len(), 2);
        assert_eq!(refs[0].id, "abc123");
        assert_eq!(refs[0].vault_id, "vlt1");
        assert_eq!(refs[0].label, "GitHub");
        assert_eq!(refs[0].origin.as_deref(), Some("https://github.com"));
        assert_eq!(refs[1].id, "def456");
        assert_eq!(refs[1].origin, None);
    }

    #[test]
    fn parses_login_item_username_and_password() {
        let json = br#"{
            "id":"i1","title":"Acme","vault":{"id":"v1"},
            "urls":[{"primary":true,"href":"https://acme.test"}],
            "fields":[
                {"id":"username","purpose":"USERNAME","value":"neo"},
                {"id":"password","purpose":"PASSWORD","value":"trinity"},
                {"id":"otp","type":"OTP","totp":"123456"}
            ]
        }"#;
        let item = OpItemRef {
            id: "i1".into(),
            vault_id: "v1".into(),
            label: "Acme".into(),
            origin: Some("https://acme.test".into()),
        };
        let resolved = parse_login_item(json, &item).unwrap();
        assert_eq!(resolved.label, "Acme");
        assert_eq!(resolved.username.as_deref(), Some("neo"));
        assert_eq!(resolved.password, "trinity");
        assert_eq!(resolved.origin.as_deref(), Some("https://acme.test"));
    }

    #[test]
    fn parses_password_item_via_concealed_fallback() {
        // A standalone Password (category 005) item: no purpose fields; its secret
        // lives in a CONCEALED field. parse_login_item must still extract it.
        let json = br#"{
            "id":"p1","title":"Wifi","vault":{"id":"v1"},
            "fields":[{"id":"password","type":"CONCEALED","value":"hunter2"}]
        }"#;
        let item = OpItemRef {
            id: "p1".into(),
            vault_id: "v1".into(),
            label: "Wifi".into(),
            origin: None,
        };
        let resolved = parse_login_item(json, &item).unwrap();
        assert_eq!(resolved.label, "Wifi");
        assert_eq!(resolved.username, None);
        assert_eq!(resolved.password, "hunter2");
    }

    // These tests drive a fake `op` binary, which is a `#!/bin/sh` script made
    // executable via Unix permission bits — so they only compile/run on Unix.
    // Gating them keeps the crate's tests compiling on Windows (where the pure
    // parser tests above still run).
    #[cfg(unix)]
    mod with_fake_op {
        use super::super::*;
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::time::Duration;

        fn write_fake_op(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
            let path = dir.join("op");
            fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
            fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
            path
        }

        #[test]
        fn resolves_reference_via_cli() {
            let dir = tempfile::tempdir().unwrap();
            // Fake `op` echoes a fixed value (no trailing newline, like --no-newline).
            let op = write_fake_op(dir.path(), "printf '%s' 's3cr3t-from-op'");
            let value =
                resolve_with(op.to_str().unwrap(), "op://Private/GitHub/credential").unwrap();
            assert_eq!(value, "s3cr3t-from-op");
        }

        #[test]
        fn surfaces_cli_failure() {
            let dir = tempfile::tempdir().unwrap();
            let op = write_fake_op(dir.path(), "echo 'no such item' 1>&2; exit 1");
            let err = resolve_with(op.to_str().unwrap(), "op://x/y/z").unwrap_err();
            assert!(err.to_string().contains("failed"));
        }

        #[test]
        fn list_login_refs_parses_items() {
            let dir = tempfile::tempdir().unwrap();
            let op = write_fake_op(
                dir.path(),
                r#"printf '%s' '[{"id":"i1","title":"Acme","vault":{"id":"v1"},"urls":[{"primary":true,"href":"https://acme.test"}]}]'"#,
            );
            let refs = list_login_refs(op.to_str().unwrap()).unwrap();
            assert_eq!(refs.len(), 1);
            assert_eq!(refs[0].id, "i1");
            assert_eq!(refs[0].vault_id, "v1");
            assert_eq!(refs[0].origin.as_deref(), Some("https://acme.test"));
        }

        #[test]
        fn import_resolved_logins_lists_then_gets_each() {
            let dir = tempfile::tempdir().unwrap();
            // Branch on `$2`: `op item list` -> array; `op item get` -> one item.
            let op = write_fake_op(
                dir.path(),
                r#"if [ "$2" = "list" ]; then printf '%s' '[{"id":"i1","title":"Acme","vault":{"id":"v1"},"urls":[{"primary":true,"href":"https://acme.test"}]}]'; elif [ "$2" = "get" ]; then printf '%s' '{"id":"i1","title":"Acme","vault":{"id":"v1"},"fields":[{"purpose":"USERNAME","value":"neo"},{"purpose":"PASSWORD","value":"trinity"}]}'; fi"#,
            );
            let (logins, errors) = import_resolved_with(op.to_str().unwrap()).unwrap();
            assert_eq!(logins.len(), 1);
            assert!(errors.is_empty());
            assert_eq!(logins[0].username.as_deref(), Some("neo"));
            assert_eq!(logins[0].password, "trinity");
            assert_eq!(logins[0].origin.as_deref(), Some("https://acme.test"));
        }

        #[test]
        fn detects_connected_account_for_biometric_path() {
            let dir = tempfile::tempdir().unwrap();
            let op = write_fake_op(
                dir.path(),
                r#"printf '%s' '[{"url":"my.1password.com","email":"a@b.c","user_uuid":"U1"}]'"#,
            );
            assert!(op_has_connected_account(op.to_str().unwrap()));
        }

        #[test]
        fn no_connected_account_when_op_lists_none() {
            let dir = tempfile::tempdir().unwrap();
            let op = write_fake_op(dir.path(), "printf '%s' '[]'");
            assert!(!op_has_connected_account(op.to_str().unwrap()));
        }

        #[test]
        fn run_op_kills_a_hanging_call() {
            let dir = tempfile::tempdir().unwrap();
            // Fake `op` that hangs (mimics the app-lock hang from sdk-go#266).
            let op = write_fake_op(dir.path(), "sleep 30");
            let start = std::time::Instant::now();
            let err = run_op(op.to_str().unwrap(), &["read", "x"], Duration::from_millis(300))
                .unwrap_err();
            assert!(err.to_string().contains("timed out"));
            // Must return promptly (well under the fake's 30s sleep).
            assert!(start.elapsed() < Duration::from_secs(5));
        }

        #[test]
        fn run_op_returns_quick_output() {
            let dir = tempfile::tempdir().unwrap();
            let op = write_fake_op(dir.path(), "printf '%s' 'hello'");
            let out = run_op(op.to_str().unwrap(), &["read", "x"], Duration::from_secs(5)).unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout), "hello");
            assert!(out.status.success());
        }
    }
}
