# momo-card Reveal Bin (Part B MVP) Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

> ✅ **AS-BUILT (2026-05-31, route A).** Shipped as 7 commits (`956a0d53`…`d75045b7`), all reviewed (per-task + final security review = SHIP-READY). The plan below describes the **route-B MVP** (card never enters the agent; `momo-card reveal` → only `{last4, expiry}`). Live testing showed last4 can't actually pay, so per the user's decision a **route-A** mode was added: **`momo-card reveal --full`** emits the COMPLETE card (`{cardNumber, cvv, expMonth, expYear}`) for the agent to use, and the `pay-with-card` skill now calls `--full`. **Accepted consequence: the full PAN+CVV enter the agent context / daemon transcript.** Default `momo-card reveal` (no `--full`) still returns only `{last4, expiry}` — so Security-invariant #2 below holds for the default mode, NOT `--full` (commit `d75045b7`). The actual payment **sink** (submitting the card to a merchant/checkout) remains deferred.

**Goal:** Give the puffer agent the ability to use the user's U-card for payment by revealing the live PAN/CVV **inside a momo-owned `momo-card` bin** — the card never enters the agent's context/transcript; the agent only ever receives the last 4 digits + expiry.

**Architecture:** A momo `[[bin]]` named `momo-card` reads ucard credentials (backend base URL + Auth-Station JWT) from `~/.ucard/.creds` (written by momo at login), exchanges the JWT for a short-lived ucard `sessionToken`, GETs `/api/card/details`, then prints **only** `{last4, expiry}` and scrubs the PAN/CVV before exit. A user-level SKILL.md tells the agent to invoke it as a **single** bash command (`momo-card reveal`). momo grants `bash argv momo-card` in the agent project's `.puffer/permissions.acl` and prepends the bin's directory to the spawned daemon's `PATH`, so the call runs through the puffer permission gate cleanly. **No puffer source is touched and no recompile is required** — the bin is built by momo, the skill is user-level (auto-discovered), and the ACL/creds are runtime files.

**Tech Stack:** Rust (reqwest blocking + serde + anyhow) for the bin and the creds/ACL writers; momo 1431 WebSocket RPC (`src-tauri/src/backend.rs`); Svelte 5 / TypeScript frontend (`daemonAuth.ts`); puffer ACL file format (`.puffer/permissions.acl`).

**Out of scope (do NOT touch):**
- **Part A (book-by-phone / `~/.wr/.creds` / full-access)** — already working; leave `apps/momo/src/lib/agent/daemonChat.ts` `permissionMode: "full-access"` as-is.
- **The payment sink** — `momo-card reveal` only proves the secure reveal chain and returns `{last4, expiry}`. Actually submitting the card to a payment target (`--to <sink>`) is a later plan, written when a concrete payment scenario exists.

**Security invariants (must hold at every step):**
1. PAN and CVV exist **only** inside the `momo-card` process. Never printed, never logged, never written to any file.
2. The bin's stdout is **only** `{"last4": "...", "expiry": "MM/YYYY"}`.
3. `~/.ucard/.creds` holds the backend base URL + the Auth-Station JWT (which can mint card-reveal sessions) — mode `0600`, same sensitivity tier as the existing `~/.wr/.creds`. It must **never** contain PAN/CVV.

---

## File Structure

| File | Responsibility | Action |
|---|---|---|
| `apps/momo/src-tauri/src/ucard_creds.rs` | Render + write `~/.ucard/.creds` (base URL + JWT, 0600). Mirror of `wr_creds.rs`. | Create |
| `apps/momo/src-tauri/src/lib.rs` | Register `mod ucard_creds;` + `mod card_agent_env;` | Modify |
| `apps/momo/src-tauri/src/backend.rs` | RPC dispatch + handlers: `write_ucard_creds`, `install_card_agent_assets` | Modify |
| `apps/momo/src-tauri/src/card_agent_env.rs` | Write the ACL allow rule + install the user-level skill file | Create |
| `apps/momo/src-tauri/src/bin/momo-card.rs` | The reveal bin: creds → exchange → details → print `{last4, expiry}`, scrub | Create |
| `apps/momo/src-tauri/Cargo.toml` | Declare the `momo-card` `[[bin]]` target | Modify |
| `apps/momo/src-tauri/src/daemon_launcher.rs` | Prepend the `momo-card` dir to the spawned daemon's `PATH` | Modify |
| `apps/momo/src/lib/agent/daemonAuth.ts` | At login: `write_ucard_creds` + `install_card_agent_assets` | Modify |

The SKILL.md text is embedded as a Rust string constant in `card_agent_env.rs` (so momo can install it to `~/.puffer/resources/skills/pay-with-card/SKILL.md` at runtime without shipping a separate asset).

---

## Task 1: ucard creds writer (`ucard_creds.rs`)

**Files:**
- Create: `apps/momo/src-tauri/src/ucard_creds.rs`
- Modify: `apps/momo/src-tauri/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Create `apps/momo/src-tauri/src/ucard_creds.rs` with the test module first:

```rust
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml ucard_creds`
Expected: FAIL — `cannot find function render_creds` (the impl isn't written yet).

- [ ] **Step 3: Write the implementation (prepend above the test module)**

```rust
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
```

- [ ] **Step 4: Register the module in `lib.rs`**

Find the line `mod wr_creds;` in `apps/momo/src-tauri/src/lib.rs` and add directly below it:

```rust
mod ucard_creds;
mod card_agent_env;
```

(`card_agent_env` is created in Task 6; declaring it now keeps the two new modules together. If the crate fails to build because `card_agent_env` doesn't exist yet, create an empty `apps/momo/src-tauri/src/card_agent_env.rs` placeholder containing only `// filled in Task 6` and finish it in Task 6.)

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml ucard_creds`
Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add apps/momo/src-tauri/src/ucard_creds.rs apps/momo/src-tauri/src/lib.rs
git commit -m "feat(momo): ucard_creds writer for ~/.ucard/.creds (base + JWT, 0600)"
```

---

## Task 2: `write_ucard_creds` 1431 RPC (backend.rs)

**Files:**
- Modify: `apps/momo/src-tauri/src/backend.rs` (dispatch arm near line 145; handler near the `write_wr_creds` fn)

- [ ] **Step 1: Add the dispatch arm**

In `apps/momo/src-tauri/src/backend.rs`, find the existing arm:

```rust
            "write_wr_creds" => {
                let api_key = string_param(&params, &["apiKey", "api_key"])?;
                let base_url = string_param(&params, &["baseUrl", "base_url"])?;
                self.write_wr_creds(&api_key, &base_url)
            }
```

Add directly below it:

```rust
            "write_ucard_creds" => {
                let base_url = string_param(&params, &["baseUrl", "base_url"])?;
                let jwt = string_param(&params, &["jwt", "accessToken", "access_token"])?;
                self.write_ucard_creds(&base_url, &jwt)
            }
```

- [ ] **Step 2: Add the handler**

Find the existing handler:

```rust
    fn write_wr_creds(&self, api_key: &str, base_url: &str) -> Result<Value> {
        let dir = home_dir().join(".wr");
        crate::wr_creds::write_creds_file(&dir, api_key, base_url)
            .with_context(|| format!("writing wr creds under {}", dir.display()))?;
        Ok(json!({ "written": true }))
    }
```

Add directly below it:

```rust
    fn write_ucard_creds(&self, base_url: &str, jwt: &str) -> Result<Value> {
        let dir = home_dir().join(".ucard");
        crate::ucard_creds::write_creds_file(&dir, base_url, jwt)
            .with_context(|| format!("writing ucard creds under {}", dir.display()))?;
        Ok(json!({ "written": true }))
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: Finishes with no errors (warnings OK).

- [ ] **Step 4: Commit**

```bash
git add apps/momo/src-tauri/src/backend.rs
git commit -m "feat(momo): write_ucard_creds 1431 RPC (mirrors write_wr_creds)"
```

---

## Task 3: write ucard creds at login (frontend)

**Files:**
- Modify: `apps/momo/src/lib/agent/daemonAuth.ts`

**Context:** `loginWorldRouter(apiKey)` already runs at login and writes `~/.wr/.creds` via `backendRequest("write_wr_creds", …)`. We add a sibling call that writes `~/.ucard/.creds`. The JWT comes from `getAuthToken()` (`apps/momo/src/lib/auth.svelte.ts:295`, localStorage key `puffer.authToken`). The ucard base URL is the **real** backend URL (`VITE_BACKEND_BASE_URL`) — the bin is not a browser, so it hits the backend directly (no CORS, no Vite proxy). Fall back to `http://127.0.0.1:8080`, the same default `backendFetch.ts` uses in production.

- [ ] **Step 1: Add the import for `getAuthToken`**

At the top of `apps/momo/src/lib/agent/daemonAuth.ts`, find the existing import line:

```ts
import { request as backendRequest } from "../wsClient";
```

Add directly below it:

```ts
import { getAuthToken } from "../auth.svelte";
```

- [ ] **Step 2: Write the ucard creds inside `loginWorldRouter`**

In `loginWorldRouter`, find the existing `try { await backendRequest("write_wr_creds", …) } catch …` block. Add this directly after that block (still inside the function):

```ts
  // Also persist the ucard backend base URL + the Auth-Station JWT to
  // ~/.ucard/.creds so the `momo-card` bin can reveal the card off-agent.
  // The base URL is the REAL backend (the bin isn't a browser → no CORS / no
  // Vite proxy), so we use VITE_BACKEND_BASE_URL directly. Best-effort.
  const ucardBase =
    (import.meta.env.VITE_BACKEND_BASE_URL as string | undefined) ??
    "http://127.0.0.1:8080";
  const jwt = getAuthToken();
  if (jwt) {
    try {
      await backendRequest("write_ucard_creds", { baseUrl: ucardBase, jwt });
      await backendRequest("install_card_agent_assets", {});
    } catch (e) {
      console.warn("[ucard] write_ucard_creds/install assets failed", e);
    }
  } else {
    console.warn("[ucard] no JWT at login — skipping ucard creds write");
  }
```

(`install_card_agent_assets` is the RPC added in Task 6; it is idempotent and does not need the JWT, but calling it here guarantees it runs once the user is signed in.)

- [ ] **Step 3: Verify types**

Run: `cd apps/momo && npm run check`
Expected: no new TypeScript errors from `daemonAuth.ts`.

- [ ] **Step 4: Commit**

```bash
git add apps/momo/src/lib/agent/daemonAuth.ts
git commit -m "feat(momo): write ucard creds + install card agent assets at login"
```

---

## Task 4: the `momo-card` reveal bin

**Files:**
- Create: `apps/momo/src-tauri/src/bin/momo-card.rs`
- Modify: `apps/momo/src-tauri/Cargo.toml`

- [ ] **Step 1: Declare the bin target in `Cargo.toml`**

In `apps/momo/src-tauri/Cargo.toml`, after the `[dependencies]` block (before `[dev-dependencies]`), add:

```toml
[[bin]]
name = "momo-card"
path = "src/bin/momo-card.rs"
```

- [ ] **Step 2: Write the bin with its pure-logic tests**

Create `apps/momo/src-tauri/src/bin/momo-card.rs`:

```rust
//! `momo-card` — reveal a U-card's live PAN/CVV INSIDE THIS PROCESS, then print
//! ONLY the non-sensitive summary (`{"last4","expiry"}`). The full card number
//! and CVV never leave this process: not printed, not logged, scrubbed before
//! exit. The agent that runs this bin via bash only ever sees the summary.
//!
//! Auth: reads `~/.ucard/.creds` (written by momo at login) for the backend
//! base URL + Auth-Station JWT, exchanges the JWT for a short-lived ucard
//! sessionToken (POST /api/auth/exchange), then GETs /api/card/details.
//!
//! Invoked as a SINGLE bash command (`momo-card reveal [--card-id N]`) so the
//! puffer permission gate (which forces approval on compound commands with
//! `&&`/`|`) does not trip; momo grants `bash argv momo-card` in the project
//! ACL so it runs without escalation.

use std::io::Write;

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};

fn main() {
    if let Err(e) = run() {
        // Terse error to stderr. No card data is ever held at a point an error
        // can occur before reveal; after reveal we scrub before any early exit.
        eprintln!("momo-card: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("reveal") => reveal(&args[2..]),
        Some(other) => bail!("unknown subcommand `{other}` (expected `reveal`)"),
        None => bail!("usage: momo-card reveal [--card-id <id>]"),
    }
}

struct Creds {
    base_url: String,
    jwt: String,
}

/// Parse `KEY=VALUE` lines. Tolerates blank lines and a trailing newline.
fn parse_creds(body: &str) -> Result<Creds> {
    let mut base_url = None;
    let mut jwt = None;
    for line in body.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "UCARD_BASE_URL" => base_url = Some(v.trim().to_string()),
                "UCARD_JWT" => jwt = Some(v.trim().to_string()),
                _ => {}
            }
        }
    }
    Ok(Creds {
        base_url: base_url
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("UCARD_BASE_URL missing from ~/.ucard/.creds"))?,
        jwt: jwt
            .filter(|s| !s.is_empty())
            .ok_or_else(|| anyhow!("UCARD_JWT missing from ~/.ucard/.creds (sign in to momo)"))?,
    })
}

fn read_creds() -> Result<Creds> {
    let home = std::env::var_os("HOME").context("HOME not set")?;
    let path = std::path::Path::new(&home).join(".ucard").join(".creds");
    let body = std::fs::read_to_string(&path)
        .with_context(|| format!("reading {} (sign in to momo first)", path.display()))?;
    parse_creds(&body)
}

#[derive(Deserialize)]
struct Envelope {
    code: i64,
    #[serde(default)]
    message: String,
    #[serde(default)]
    data: Value,
}

/// Decode the ucard-backend envelope `{code,message,field,data}`. A non-zero
/// code is an error; we surface `code` + `message` only — never the raw body,
/// which on these endpoints is card-adjacent.
fn unwrap_envelope(raw: &str) -> Result<Value> {
    let env: Envelope =
        serde_json::from_str(raw).context("backend returned a non-envelope response")?;
    if env.code != 0 {
        bail!("backend error code={} {}", env.code, env.message);
    }
    Ok(env.data)
}

/// Last 4 digits of a PAN (digits only). Empty string if fewer than 4 digits.
fn last4(pan: &str) -> String {
    let digits: String = pan.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 4 {
        digits[digits.len() - 4..].to_string()
    } else {
        String::new()
    }
}

/// "MM/YYYY" from the live expMonth/expYear; empty string if either is missing.
fn expiry(month: &str, year: &str) -> String {
    if !month.is_empty() && !year.is_empty() {
        format!("{month}/{year}")
    } else {
        String::new()
    }
}

fn exchange(client: &reqwest::blocking::Client, base: &str, jwt: &str) -> Result<String> {
    let url = format!("{base}/api/auth/exchange");
    let text = client
        .post(&url)
        .json(&json!({ "accessToken": jwt }))
        .send()
        .context("POST /api/auth/exchange failed")?
        .text()
        .context("reading /api/auth/exchange response")?;
    let data = unwrap_envelope(&text)?;
    data.get("sessionToken")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| anyhow!("exchange returned no sessionToken"))
}

/// First card's id (string or number) from /api/card/list — used when the
/// caller doesn't pass --card-id.
fn first_card_id(client: &reqwest::blocking::Client, base: &str, session: &str) -> Result<i64> {
    let url = format!("{base}/api/card/list");
    let text = client
        .get(&url)
        .bearer_auth(session)
        .send()
        .context("GET /api/card/list failed")?
        .text()
        .context("reading /api/card/list response")?;
    let data = unwrap_envelope(&text)?;
    let first = data
        .get("cards")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .ok_or_else(|| anyhow!("no cards on this account"))?;
    let id = first.get("id").ok_or_else(|| anyhow!("card has no id"))?;
    if let Some(n) = id.as_i64() {
        Ok(n)
    } else if let Some(s) = id.as_str() {
        s.parse().context("card id is not an integer")
    } else {
        bail!("card id has unexpected type")
    }
}

struct Card {
    number: String,
    cvv: String,
    exp_month: String,
    exp_year: String,
}

fn card_details(
    client: &reqwest::blocking::Client,
    base: &str,
    session: &str,
    card_id: i64,
) -> Result<Card> {
    let url = format!("{base}/api/card/details?cardId={card_id}");
    let text = client
        .get(&url)
        .bearer_auth(session)
        .send()
        .context("GET /api/card/details failed")?
        .text()
        .context("reading /api/card/details response")?;
    let data = unwrap_envelope(&text)?;
    let get = |k: &str| data.get(k).and_then(Value::as_str).unwrap_or("").to_string();
    Ok(Card {
        number: get("cardNumber"),
        cvv: get("cvv"),
        exp_month: get("expMonth"),
        exp_year: get("expYear"),
    })
}

fn reveal(args: &[String]) -> Result<()> {
    let mut card_id: Option<i64> = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--card-id" => {
                let v = args.get(i + 1).context("--card-id needs a value")?;
                card_id = Some(v.parse().context("--card-id must be an integer")?);
                i += 2;
            }
            other => bail!("unknown flag `{other}`"),
        }
    }

    let creds = read_creds()?;
    let client = reqwest::blocking::Client::builder()
        .build()
        .context("building HTTP client")?;
    let session = exchange(&client, &creds.base_url, &creds.jwt)?;
    let id = match card_id {
        Some(id) => id,
        None => first_card_id(&client, &creds.base_url, &session)?,
    };
    let mut card = card_details(&client, &creds.base_url, &session, id)?;

    // Build ONLY the non-sensitive summary, then scrub the sensitive strings.
    let summary = json!({
        "last4": last4(&card.number),
        "expiry": expiry(&card.exp_month, &card.exp_year),
    });
    card.number.clear();
    card.cvv.clear();
    card.exp_month.clear();
    card.exp_year.clear();
    drop(card);

    let mut out = std::io::stdout();
    out.write_all(summary.to_string().as_bytes())?;
    out.write_all(b"\n")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_creds_reads_both_vars() {
        let c = parse_creds("UCARD_BASE_URL=https://x\nUCARD_JWT=jwt-1\n").unwrap();
        assert_eq!(c.base_url, "https://x");
        assert_eq!(c.jwt, "jwt-1");
    }

    #[test]
    fn parse_creds_errors_when_jwt_missing() {
        let err = parse_creds("UCARD_BASE_URL=https://x\n").unwrap_err();
        assert!(err.to_string().contains("UCARD_JWT"));
    }

    #[test]
    fn last4_takes_trailing_digits_only() {
        assert_eq!(last4("4111 1111 1111 1234"), "1234");
        assert_eq!(last4("12"), "");
    }

    #[test]
    fn expiry_joins_or_empties() {
        assert_eq!(expiry("04", "2028"), "04/2028");
        assert_eq!(expiry("", "2028"), "");
    }

    #[test]
    fn unwrap_envelope_rejects_nonzero_code() {
        let err = unwrap_envelope(r#"{"code":1005,"message":"bad","data":{}}"#).unwrap_err();
        assert!(err.to_string().contains("1005"));
    }

    #[test]
    fn unwrap_envelope_returns_data_on_zero() {
        let data = unwrap_envelope(r#"{"code":0,"message":"","data":{"sessionToken":"t"}}"#).unwrap();
        assert_eq!(data.get("sessionToken").unwrap().as_str().unwrap(), "t");
    }
}
```

- [ ] **Step 3: Build the bin and run its tests**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml --bin momo-card`
Expected: PASS (6 tests).

Run: `cargo build --manifest-path apps/momo/src-tauri/Cargo.toml --bin momo-card`
Expected: produces `apps/momo/src-tauri/target/debug/momo-card`.

- [ ] **Step 4: Commit**

```bash
git add apps/momo/src-tauri/src/bin/momo-card.rs apps/momo/src-tauri/Cargo.toml
git commit -m "feat(momo): momo-card reveal bin — card PAN/CVV stay in-process, prints only last4+expiry"
```

---

## Task 5: put `momo-card` on the spawned daemon's PATH

**Files:**
- Modify: `apps/momo/src-tauri/src/daemon_launcher.rs`

**Context:** The agent runs `momo-card reveal` via bash inside the daemon. For the puffer ACL to match `bash argv momo-card`, the command's first token's basename must be literally `momo-card` — so the bin must be resolvable by name on the daemon's `PATH` (a `$VAR`-path would not match the ACL, which parses the unexpanded command). The daemon inherits momo's environment; we prepend the directory holding `momo-card` to `PATH`. `momo-card` is built into the same `target/<profile>/` dir as the `puffer` binary that `resolve_puffer_binary` already finds, so we reuse that resolution and take its parent directory.

- [ ] **Step 1: Add a helper that resolves the `momo-card` directory**

In `apps/momo/src-tauri/src/daemon_launcher.rs`, directly below the `resolve_puffer_binary` function, add:

```rust
/// Directory that should contain the `momo-card` bin (built into the same
/// `target/<profile>/` as `puffer`). We reuse `resolve_puffer_binary`'s
/// location logic and return its parent dir; the daemon's PATH gets this
/// prepended so the agent can run `momo-card` by name (required for the
/// `bash argv momo-card` ACL match).
fn resolve_momo_card_dir() -> Option<PathBuf> {
    let puffer = resolve_puffer_binary().ok()?;
    puffer.parent().map(Path::to_path_buf)
}
```

If `Path` is not already imported at the top of the file, add `use std::path::Path;` to the existing `use std::path::PathBuf;` import (change it to `use std::path::{Path, PathBuf};`).

- [ ] **Step 2: Prepend the dir to the daemon's PATH in `spawn_daemon`**

In `spawn_daemon`, find this block:

```rust
    if std::env::var_os("PUFFER_BUILTIN_RESOURCES_DIR").is_none() {
        if let Some(resources_dir) = resolve_builtin_resources_dir(&binary) {
            cmd.env("PUFFER_BUILTIN_RESOURCES_DIR", resources_dir);
        }
    }
```

Add directly after it (before `let mut child = cmd.spawn()`):

```rust
    // Make the `momo-card` bin reachable by name on the daemon's PATH, so the
    // agent's single-command `momo-card reveal` resolves and matches the
    // `bash argv momo-card` project ACL rule.
    if let Some(card_dir) = resolve_momo_card_dir() {
        let existing = std::env::var("PATH").unwrap_or_default();
        let sep = if cfg!(windows) { ";" } else { ":" };
        cmd.env("PATH", format!("{}{sep}{existing}", card_dir.display()));
    }
```

- [ ] **Step 3: Verify it compiles**

Run: `cargo check --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: no errors.

- [ ] **Step 4: Commit**

```bash
git add apps/momo/src-tauri/src/daemon_launcher.rs
git commit -m "feat(momo): prepend momo-card dir to daemon PATH for ACL-matchable invocation"
```

---

## Task 6: ACL grant + user-level skill install (`card_agent_env.rs`)

**Files:**
- Create/replace: `apps/momo/src-tauri/src/card_agent_env.rs` (placeholder created in Task 1)
- Modify: `apps/momo/src-tauri/src/backend.rs` (dispatch arm + handler)

**Context:**
- **ACL grant.** puffer loads project ACL rules from `<session-cwd>/.puffer/permissions.acl` (`ConfigPaths::discover(cwd).workspace_config_dir = cwd.join(".puffer")`). momo's agent session cwd is `app_home()/projects/default` (`backend.rs:198`). The line `100 allow bash argv momo-card` grants the bin. Format: `<priority> <allow|deny> bash argv <name>` (`acl.rs::parse_acl_scope`/`parse_bash_scope`).
- **Skill.** User-level skills live at `~/.puffer/resources/skills/<name>/SKILL.md` and are auto-discovered + hot-reloaded — no `puffer-builtins.yaml`, no recompile. The daemon workspace is `$HOME`, so its user config root is `~/.puffer`.
- Both are idempotent (we overwrite/ensure the exact content), so re-running on every login is safe.

- [ ] **Step 1: Write `card_agent_env.rs` with its tests**

Replace the contents of `apps/momo/src-tauri/src/card_agent_env.rs` with:

```rust
//! Installs the agent-side assets the `momo-card` reveal flow needs:
//!   1. A project ACL rule (`100 allow bash argv momo-card`) so the bin runs
//!      through the puffer permission gate without escalation.
//!   2. A user-level `pay-with-card` skill that tells the agent to call the
//!      bin as a SINGLE command and that it only ever receives last4 + expiry.
//!
//! Both are idempotent: the ACL line is added only if absent; the SKILL.md is
//! overwritten with the canonical text.

use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

const ACL_LINE: &str = "100 allow bash argv momo-card";

const SKILL_NAME: &str = "pay-with-card";

const SKILL_MD: &str = r#"---
name: pay-with-card
description: Reveal the user's saved U-card for a payment the current task requires. Use ONLY when a step genuinely needs the user's card (a checkout, a deposit/guarantee, a paid booking that asks for a card). Returns just the last 4 digits and expiry — the full card number and CVV are handled securely by the app and are never shown to you.
metadata:
  author: momo
  version: "1.0.0"
  clawdbot:
    emoji: "💳"
---

# Pay with Card

When the task you are doing needs the user's payment card, run this **single**
command (no pipes, no `&&`, no redirection):

```bash
momo-card reveal
```

It prints a JSON summary and nothing else, e.g.:

```json
{"last4": "1234", "expiry": "04/2028"}
```

## What this does (and does not) give you

- You receive **only** `last4` + `expiry`. This is by design and is enough to
  confirm to the user which card was used ("paid with the card ending 1234").
- You will **never** receive the full card number or CVV. The `momo-card` bin
  reveals them inside its own process, uses them, and discards them. Do not ask
  the user for the full number or CVV, and do not try to reconstruct them.
- If the command fails with "sign in to momo first", tell the user to sign in.

## When to use

- A checkout / payment form the task must complete.
- A booking or reservation that requires a card to hold or guarantee it.
- A deposit/top-up that needs a card.

Do **not** run it speculatively — only when a concrete payment step requires it.
"#;

/// Append `100 allow bash argv momo-card` to `<project_cwd>/.puffer/permissions.acl`
/// if an identical line isn't already present. Creates the dir/file as needed.
pub fn ensure_acl_grant(project_cwd: &Path) -> Result<()> {
    let dir = project_cwd.join(".puffer");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("permissions.acl");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    if existing.lines().any(|l| l.trim() == ACL_LINE) {
        return Ok(());
    }
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .with_context(|| format!("opening {}", path.display()))?;
    // Ensure we start on a fresh line if the file didn't end with one.
    if !existing.is_empty() && !existing.ends_with('\n') {
        f.write_all(b"\n")?;
    }
    f.write_all(ACL_LINE.as_bytes())?;
    f.write_all(b"\n")?;
    Ok(())
}

/// Write the canonical SKILL.md to `<user_puffer>/resources/skills/pay-with-card/SKILL.md`.
pub fn install_skill(user_puffer: &Path) -> Result<()> {
    let dir = user_puffer
        .join("resources")
        .join("skills")
        .join(SKILL_NAME);
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating {}", dir.display()))?;
    let path = dir.join("SKILL.md");
    std::fs::write(&path, SKILL_MD).with_context(|| format!("writing {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn acl_grant_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        ensure_acl_grant(cwd).unwrap();
        ensure_acl_grant(cwd).unwrap();
        let body = std::fs::read_to_string(cwd.join(".puffer").join("permissions.acl")).unwrap();
        assert_eq!(body.matches(ACL_LINE).count(), 1);
    }

    #[test]
    fn acl_grant_preserves_existing_lines() {
        let tmp = tempfile::tempdir().unwrap();
        let cwd = tmp.path();
        let dir = cwd.join(".puffer");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("permissions.acl"), "200 allow read dir foo").unwrap();
        ensure_acl_grant(cwd).unwrap();
        let body = std::fs::read_to_string(dir.join("permissions.acl")).unwrap();
        assert!(body.contains("200 allow read dir foo"));
        assert!(body.contains(ACL_LINE));
    }

    #[test]
    fn install_skill_writes_skill_md() {
        let tmp = tempfile::tempdir().unwrap();
        install_skill(tmp.path()).unwrap();
        let path = tmp
            .path()
            .join("resources/skills/pay-with-card/SKILL.md");
        let body = std::fs::read_to_string(&path).unwrap();
        assert!(body.contains("name: pay-with-card"));
        assert!(body.contains("momo-card reveal"));
    }
}
```

- [ ] **Step 2: Run the tests to verify they pass**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml card_agent_env`
Expected: PASS (3 tests).

- [ ] **Step 3: Add the `install_card_agent_assets` dispatch arm (backend.rs)**

In `apps/momo/src-tauri/src/backend.rs`, directly below the `"write_ucard_creds" => { … }` arm added in Task 2, add:

```rust
            "install_card_agent_assets" => self.install_card_agent_assets(),
```

- [ ] **Step 4: Add the handler (backend.rs)**

Directly below the `write_ucard_creds` handler added in Task 2, add:

```rust
    fn install_card_agent_assets(&self) -> Result<Value> {
        let project_cwd = app_home()?.join("projects").join("default");
        crate::card_agent_env::ensure_acl_grant(&project_cwd)
            .with_context(|| format!("granting momo-card ACL under {}", project_cwd.display()))?;
        let user_puffer = home_dir().join(".puffer");
        crate::card_agent_env::install_skill(&user_puffer)
            .with_context(|| format!("installing pay-with-card skill under {}", user_puffer.display()))?;
        Ok(json!({ "installed": true }))
    }
```

- [ ] **Step 5: Verify it compiles + full unit tests pass**

Run: `cargo test --manifest-path apps/momo/src-tauri/Cargo.toml`
Expected: all tests pass (ucard_creds + momo-card + card_agent_env).

- [ ] **Step 6: Commit**

```bash
git add apps/momo/src-tauri/src/card_agent_env.rs apps/momo/src-tauri/src/backend.rs
git commit -m "feat(momo): install pay-with-card skill + grant momo-card project ACL"
```

---

## Task 7: end-to-end verification

> No new code. Manual verification that the secure chain works and the bin
> resolves. Do not modify Part A.

- [ ] **Step 1: Build everything**

```bash
cargo build --manifest-path apps/momo/src-tauri/Cargo.toml --bin momo-card
cargo build --manifest-path apps/momo/src-tauri/Cargo.toml
```
Expected: both succeed; `apps/momo/src-tauri/target/debug/momo-card` exists.

- [ ] **Step 2: Run momo, sign in, confirm assets were written**

Start momo (the dev flow you normally use), sign in, then check:

```bash
test -f ~/.ucard/.creds && stat -f '%Sp' ~/.ucard/.creds        # expect -rw-------
cat ~/.momo/projects/default/.puffer/permissions.acl            # expect a line: 100 allow bash argv momo-card
test -f ~/.puffer/resources/skills/pay-with-card/SKILL.md && echo skill-ok
```
Expected: creds file is `0600`, the ACL line is present, the skill file exists.
(If you use `MOMO_HOME`, substitute it for `~/.momo`.)

- [ ] **Step 3: Functional reveal (card stays off-agent)**

In a momo chat, prompt something that needs payment, e.g.:
> "Use my saved card to pay — just confirm which card."

Assert:
- The agent runs **`momo-card reveal`** as a single bash command.
- The agent's reply / the transcript shows only a `last4` + `expiry` (e.g. `1234`, `04/2028`).
- Search the live DevTools + the daemon session transcript (`~/.puffer/...`) for the full PAN/CVV — there must be **no** full card number anywhere.

- [ ] **Step 4: (Optional, local-only) verify the ACL works without full-access**

> This temporarily turns OFF Part A's full-access ONLY to confirm the ACL grant
> is what allows the bin — then reverts. book-by-phone will be gated during this
> check; that's expected. Do NOT commit this change.

1. In `apps/momo/src/lib/agent/daemonChat.ts`, temporarily change `permissionMode: options.permissionMode ?? "full-access"` to `?? "workspace-write"`.
2. Restart momo, repeat Step 3. Expected: `momo-card reveal` still runs **without a permission prompt** (the ACL allows it). A control command like `whoami` would instead prompt — confirming the gate is active and it's the ACL letting `momo-card` through.
3. **Revert** the change (`git checkout apps/momo/src/lib/agent/daemonChat.ts`). Confirm `git status` shows it clean.

- [ ] **Step 5: Final commit (docs/status only, if anything)**

No production code changes here. If you kept notes, commit only intended files.

---

## Self-Review

**1. Spec coverage:**
- Card revealed off-agent, only last4+expiry to agent → Task 4 (bin) + Task 7 Step 3. ✓
- Auth via JWT→sessionToken→/card/details, creds on disk → Task 1/2/3 + Task 4. ✓
- Single-command, ACL-allowed, no full-access dependency → Task 5 (PATH) + Task 6 (ACL) + Task 7 Step 4. ✓
- No puffer recompile (momo bin + user-level skill) → Task 4 (momo `[[bin]]`) + Task 6 (`install_skill` to `~/.puffer/resources/skills/`). ✓
- Part A untouched → no task modifies `daemonChat.ts` (except the temporary, reverted, optional Step 4) or `~/.wr/.creds` / book-by-phone. ✓
- Sink deferred → bin has no `--to`; noted out-of-scope. ✓

**2. Placeholder scan:** No "TBD"/"add error handling"/"similar to Task N" — every code step has complete code. ✓

**3. Type/name consistency:**
- Creds vars `UCARD_BASE_URL` / `UCARD_JWT` consistent across `ucard_creds.rs` (Task 1), the bin's `parse_creds` (Task 4), and the login write (Task 3). ✓
- RPC names `write_ucard_creds` / `install_card_agent_assets` consistent across backend dispatch+handlers (Task 2/6) and the frontend calls (Task 3). ✓
- ACL line `100 allow bash argv momo-card` consistent between `card_agent_env.rs` (Task 6) and the PATH/bin name `momo-card` (Task 4/5). ✓
- Bin output keys `last4` / `expiry` consistent between the bin (Task 4) and the skill text + verification (Task 6/7). ✓

**Note carried for a future plan:** an actual payment **sink** (`momo-card --to <target>`) and, separately, bin-ifying book-by-phone to revert Part A's full-access. Both are deliberately out of this plan's scope.
