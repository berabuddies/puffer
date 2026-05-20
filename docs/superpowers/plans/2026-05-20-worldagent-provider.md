# worldagent Provider Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Add a new `worldagent` provider entry to Puffer that supports both API-key paste and an Auth-Station OAuth login flow (opens https://auth-worldrouter.vercel.app/login in the system browser, captures token+refresh on a fixed localhost callback, stores credential).

**Architecture:** A new minimal Rust crate `puffer-provider-worldagent` implements Auth Station's "token-in-redirect" flow (no PKCE, no code exchange). `ProviderDescriptor`/`ProviderPack` gain an optional `oauth_family` field that lets a provider opt into a non-`default_api`-derived OAuth handler. `puffer-cli`'s OAuth dispatch (`auth_provider.rs` + `daemon.rs` + `main.rs`) grows a third arm for `OauthFamily::WorldAgent`. `authflow::CallbackListener` grows a `bind_localhost_port` variant for fixed-port binds. A new `resources/providers/worldagent.yaml` ships the provider. The desktop Svelte UI registers a visual entry; LoginView is unchanged.

**Tech Stack:** Rust (workspace crates), reqwest blocking client, base64 + serde_json (JWT payload decode), serde_yaml (provider yaml), Svelte 5 + TypeScript (desktop visual entry).

**Spec:** `docs/superpowers/specs/2026-05-20-worldagent-provider-design.md`

---

## File Structure

**Created:**
- `crates/puffer-provider-worldagent/Cargo.toml`
- `crates/puffer-provider-worldagent/src/lib.rs`
- `crates/puffer-provider-worldagent/src/auth.rs`
- `resources/providers/worldagent.yaml`
- `specs/puffer-provider-worldagent/00.md`
- `specs/puffer-provider-registry/06.md`
- `specs/puffer-cli/<next>.md` — exact filename picked in Task 8

**Modified:**
- `Cargo.toml` (workspace members + workspace deps)
- `crates/puffer-provider-registry/src/model.rs` — `oauth_family` field
- `crates/puffer-resources/src/model.rs` — `ProviderPack.oauth_family` mirror + `into_descriptor` pass-through
- `crates/puffer-cli/Cargo.toml` — dep on `puffer-provider-worldagent`
- `crates/puffer-cli/src/auth_provider.rs` — `OauthFamily::WorldAgent` arm
- `crates/puffer-cli/src/auth_credentials.rs` — `to_registry_oauth_credential_worldagent`
- `crates/puffer-cli/src/authflow.rs` — `bind_localhost_port` helper
- `crates/puffer-cli/src/main.rs` — login flow + `run_login_flow` arm
- `crates/puffer-cli/src/daemon.rs` — `handle_login_with_oauth` arm
- `apps/puffer-desktop/src/lib/providerVisuals.ts` — visual entry for `worldagent`

---

## Task 1: Add `oauth_family` field to ProviderDescriptor + ProviderPack

**Files:**
- Modify: `crates/puffer-provider-registry/src/model.rs`
- Modify: `crates/puffer-resources/src/model.rs`

- [ ] **Step 1: Write the failing test in puffer-provider-registry**

Append to `crates/puffer-provider-registry/src/model.rs` test module (the file already ends with `}` for impls; add a `#[cfg(test)] mod` if absent — currently the file has no test module, add one at the very end):

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml;

    #[test]
    fn provider_descriptor_deserializes_oauth_family_field() {
        let yaml = r#"
id: example
display_name: Example
base_url: https://example.invalid
default_api: openai-completions
oauth_family: worldagent
auth_modes:
  - oauth
"#;
        let provider: ProviderDescriptor =
            serde_yaml::from_str(yaml).expect("provider yaml parses");
        assert_eq!(provider.oauth_family.as_deref(), Some("worldagent"));
    }

    #[test]
    fn provider_descriptor_oauth_family_defaults_to_none() {
        let yaml = r#"
id: example
display_name: Example
base_url: https://example.invalid
default_api: openai-completions
auth_modes:
  - oauth
"#;
        let provider: ProviderDescriptor =
            serde_yaml::from_str(yaml).expect("provider yaml parses");
        assert!(provider.oauth_family.is_none());
    }
}
```

If `serde_yaml` isn't already a dev-dep, add it. Check first:

```bash
grep -n "serde_yaml\|serde-yaml" crates/puffer-provider-registry/Cargo.toml
```

If absent, add to `[dev-dependencies]`:

```toml
[dev-dependencies]
serde_yaml = "0.9"
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-provider-registry provider_descriptor_deserializes_oauth_family_field
```

Expected: FAIL — field doesn't exist on `ProviderDescriptor` (unknown field).

- [ ] **Step 3: Add the field to `ProviderDescriptor`**

In `crates/puffer-provider-registry/src/model.rs`, inside the `ProviderDescriptor` struct (around line 309-338), add right after `chat_completions_path`:

```rust
    /// Optional explicit OAuth family for this provider. When `None`,
    /// callers infer the family from `default_api` (preserving every
    /// yaml that did not opt in). When `Some`, callers use the named
    /// family directly. Known values today: `"openai"`, `"anthropic"`,
    /// `"worldagent"`. This is the seam that lets a provider whose
    /// transport is `openai-completions` use a non-OpenAI OAuth flow.
    #[serde(default)]
    pub oauth_family: Option<String>,
```

- [ ] **Step 4: Run test to verify it passes**

```bash
cargo test -p puffer-provider-registry provider_descriptor
```

Expected: PASS for both new tests.

- [ ] **Step 5: Mirror the field in `ProviderPack`**

In `crates/puffer-resources/src/model.rs`, find `pub struct ProviderPack` (around line 449) and add right after `chat_completions_path`:

```rust
    #[serde(default)]
    pub oauth_family: Option<String>,
```

Then update `into_descriptor()` (around line 472) — add `oauth_family: self.oauth_family,` in the struct literal:

```rust
    pub fn into_descriptor(self) -> ProviderDescriptor {
        ProviderDescriptor {
            id: self.id,
            display_name: self.display_name,
            base_url: self.base_url,
            default_api: self.default_api,
            auth_modes: self.auth_modes,
            headers: self.headers,
            query_params: self.query_params,
            chat_completions_path: self.chat_completions_path,
            oauth_family: self.oauth_family,
            discovery: self.discovery,
            models: self.models,
        }
    }
```

- [ ] **Step 6: Run resources build**

```bash
cargo build -p puffer-resources
```

Expected: SUCCESS.

- [ ] **Step 7: Verify existing registry callers still compile**

```bash
cargo build -p puffer-cli
```

Expected: SUCCESS. Existing yaml that doesn't set the field continues to parse (serde default = `None`).

- [ ] **Step 8: Commit**

```bash
git add crates/puffer-provider-registry/src/model.rs \
        crates/puffer-provider-registry/Cargo.toml \
        crates/puffer-resources/src/model.rs
git commit -m "feat(provider-registry): add optional oauth_family to ProviderDescriptor"
```

---

## Task 2: Bootstrap the `puffer-provider-worldagent` crate

**Files:**
- Create: `crates/puffer-provider-worldagent/Cargo.toml`
- Create: `crates/puffer-provider-worldagent/src/lib.rs`
- Modify: `Cargo.toml` (workspace `members`)

- [ ] **Step 1: Create the crate Cargo.toml**

`crates/puffer-provider-worldagent/Cargo.toml`:

```toml
[package]
name = "puffer-provider-worldagent"
version.workspace = true
edition.workspace = true
license.workspace = true

[dependencies]
anyhow.workspace = true
base64.workspace = true
rand = "0.9.2"
reqwest.workspace = true
serde.workspace = true
serde_json.workspace = true
url = "2.5.7"
```

No need to depend on `puffer-provider-registry` — this crate only owns the Auth Station auth helpers; the registry-shape conversion lives in `puffer-cli/src/auth_credentials.rs`.

- [ ] **Step 2: Create a stub lib.rs**

`crates/puffer-provider-worldagent/src/lib.rs`:

```rust
//! Auth Station OAuth helpers for the `worldagent` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

mod auth;

pub use auth::{
    build_login_url, decode_jwt_profile, exchange_jwt_for_api_key,
    generate_client_state, parse_callback_input, refresh_oauth_token,
    WorldAgentCallback, WorldAgentJwtProfile, WorldAgentLoginConfig,
    WorldAgentOAuthCredentials, WORLDAGENT_AUTH_BASE_URL,
    WORLDAGENT_AUTH_URL_OVERRIDE_ENV, WORLDAGENT_CALLBACK_PATH,
    WORLDAGENT_CALLBACK_PORT, WORLDAGENT_DEFAULT_REDIRECT_URI,
};
```

- [ ] **Step 3: Create an empty auth.rs so the crate compiles**

`crates/puffer-provider-worldagent/src/auth.rs`:

```rust
//! Auth Station login URL building, callback parsing, JWT decoding.
```

- [ ] **Step 4: Register the crate in the workspace**

In root `Cargo.toml`, find the `members = [` block and add `"crates/puffer-provider-worldagent",` keeping the list alphabetically grouped (insert right after `"crates/puffer-provider-registry",`).

- [ ] **Step 5: Build empty crate to confirm Cargo wiring**

```bash
cargo build -p puffer-provider-worldagent
```

Expected: builds with "unresolved import" errors because lib.rs re-exports items that don't exist. That's a problem — the next task implements them. For this commit, **temporarily comment out the re-exports** in lib.rs so it builds; we'll restore them in Task 3 step 6.

Replace lib.rs body for now:

```rust
//! Auth Station OAuth helpers for the `worldagent` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

#![allow(dead_code)]

mod auth;
```

```bash
cargo build -p puffer-provider-worldagent
```

Expected: SUCCESS (empty crate).

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-provider-worldagent Cargo.toml
git commit -m "feat(provider-worldagent): bootstrap empty crate"
```

---

## Task 3: Implement `WorldAgentLoginConfig` + `build_login_url`

**Files:**
- Modify: `crates/puffer-provider-worldagent/src/auth.rs`
- Modify: `crates/puffer-provider-worldagent/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to `crates/puffer-provider-worldagent/src/auth.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_login_url_contains_redirect_uri_and_client_state() {
        let config = WorldAgentLoginConfig {
            auth_base_url: "https://auth-worldrouter.vercel.app".to_string(),
            redirect_uri: "http://127.0.0.1:1456/callback".to_string(),
            client_state: "state-xyz".to_string(),
        };
        let url = build_login_url(&config);
        assert!(url.starts_with("https://auth-worldrouter.vercel.app/login?"));
        assert!(url.contains("redirect_uri=http%3A%2F%2F127.0.0.1%3A1456%2Fcallback"));
        assert!(url.contains("client_state=state-xyz"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-provider-worldagent build_login_url
```

Expected: FAIL — `WorldAgentLoginConfig` / `build_login_url` do not exist.

- [ ] **Step 3: Implement the types and function**

Replace the body of `crates/puffer-provider-worldagent/src/auth.rs` (above the test module) with:

```rust
//! Auth Station login URL building, callback parsing, JWT decoding.

use anyhow::{anyhow, Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use rand::RngCore;
use reqwest::blocking::Client;
use serde::Deserialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Default Auth Station base URL (Sandbox). Production is
/// `https://auth.worldrouter.ai`. The env var named by
/// [`WORLDAGENT_AUTH_URL_OVERRIDE_ENV`] overrides this at runtime.
pub const WORLDAGENT_AUTH_BASE_URL: &str = "https://auth-worldrouter.vercel.app";

/// Env var name that overrides the Auth Station base URL.
pub const WORLDAGENT_AUTH_URL_OVERRIDE_ENV: &str = "PUFFER_WORLDAGENT_AUTH_URL";

/// Fixed loopback callback path used by Puffer desktop. The auth
/// team must allow-list the full URI on both Sandbox and Production.
pub const WORLDAGENT_CALLBACK_PATH: &str = "/callback";

/// Fixed loopback callback port used by Puffer desktop. See
/// [`WORLDAGENT_CALLBACK_PATH`] for the path component.
pub const WORLDAGENT_CALLBACK_PORT: u16 = 1456;

/// Concatenated fixed loopback redirect URI.
pub const WORLDAGENT_DEFAULT_REDIRECT_URI: &str = "http://127.0.0.1:1456/callback";

/// Parameters needed to build an Auth Station login URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldAgentLoginConfig {
    /// Base URL of Auth Station, no trailing slash.
    pub auth_base_url: String,
    /// Full redirect URI for the desktop callback listener.
    pub redirect_uri: String,
    /// Opaque random value used as the CSRF guard.
    pub client_state: String,
}

impl Default for WorldAgentLoginConfig {
    fn default() -> Self {
        let auth_base_url = std::env::var(WORLDAGENT_AUTH_URL_OVERRIDE_ENV)
            .ok()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| WORLDAGENT_AUTH_BASE_URL.to_string());
        Self {
            auth_base_url,
            redirect_uri: WORLDAGENT_DEFAULT_REDIRECT_URI.to_string(),
            client_state: generate_client_state(),
        }
    }
}

/// Generates an opaque url-safe random `client_state`.
pub fn generate_client_state() -> String {
    let mut bytes = [0_u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

/// Builds the Auth Station `/login` URL for the given config.
pub fn build_login_url(config: &WorldAgentLoginConfig) -> String {
    let trimmed = config.auth_base_url.trim_end_matches('/');
    let mut url = url::Url::parse(&format!("{trimmed}/login"))
        .expect("auth_base_url must be a valid URL");
    url.query_pairs_mut()
        .append_pair("redirect_uri", &config.redirect_uri)
        .append_pair("client_state", &config.client_state);
    url.to_string()
}
```

- [ ] **Step 4: Restore the public re-exports in lib.rs**

Replace `crates/puffer-provider-worldagent/src/lib.rs`:

```rust
//! Auth Station OAuth helpers for the `worldagent` provider.
//!
//! Auth Station's `/login` flow returns the final `token` and
//! `refresh_token` directly in the callback URL. There is no PKCE,
//! no code exchange. This crate owns URL building, callback
//! parsing, JWT-payload decoding, and refresh.

mod auth;

pub use auth::{
    build_login_url, generate_client_state, WorldAgentLoginConfig,
    WORLDAGENT_AUTH_BASE_URL, WORLDAGENT_AUTH_URL_OVERRIDE_ENV,
    WORLDAGENT_CALLBACK_PATH, WORLDAGENT_CALLBACK_PORT,
    WORLDAGENT_DEFAULT_REDIRECT_URI,
};
```

(Other re-exports — `parse_callback_input`, `decode_jwt_profile`, `refresh_oauth_token`, `exchange_jwt_for_api_key`, `WorldAgentCallback`, `WorldAgentJwtProfile`, `WorldAgentOAuthCredentials` — are added in later tasks.)

- [ ] **Step 5: Run test**

```bash
cargo test -p puffer-provider-worldagent build_login_url
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-provider-worldagent/src
git commit -m "feat(provider-worldagent): implement build_login_url + config"
```

---

## Task 4: Implement `parse_callback_input` + `WorldAgentCallback`

**Files:**
- Modify: `crates/puffer-provider-worldagent/src/auth.rs`
- Modify: `crates/puffer-provider-worldagent/src/lib.rs`

- [ ] **Step 1: Write the failing tests**

Append to the existing test module in `auth.rs`:

```rust
    #[test]
    fn parse_callback_input_extracts_token_refresh_state() {
        let parsed = parse_callback_input(
            "http://127.0.0.1:1456/callback?token=acc&refresh_token=ref&state=xyz",
        );
        assert_eq!(parsed.token.as_deref(), Some("acc"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("ref"));
        assert_eq!(parsed.state.as_deref(), Some("xyz"));
        assert!(parsed.error.is_none());
    }

    #[test]
    fn parse_callback_input_extracts_error() {
        let parsed = parse_callback_input(
            "http://127.0.0.1:1456/callback?error=invalid_state&error_description=bad+state&state=xyz",
        );
        assert_eq!(parsed.error.as_deref(), Some("invalid_state"));
        assert_eq!(parsed.error_description.as_deref(), Some("bad state"));
        assert!(parsed.token.is_none());
    }

    #[test]
    fn parse_callback_input_handles_raw_query_string() {
        let parsed = parse_callback_input("token=acc&state=xyz");
        assert_eq!(parsed.token.as_deref(), Some("acc"));
        assert_eq!(parsed.state.as_deref(), Some("xyz"));
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p puffer-provider-worldagent parse_callback_input
```

Expected: FAIL — function does not exist.

- [ ] **Step 3: Implement the types and function**

Append below `build_login_url` (before the `#[cfg(test)]` block):

```rust
/// Parsed callback fields. Each field is `None` when its parameter
/// was absent from the callback URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldAgentCallback {
    pub token: Option<String>,
    pub refresh_token: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Extracts `token`, `refresh_token`, `state`, `error`, and
/// `error_description` from a callback URL or raw query string.
pub fn parse_callback_input(input: &str) -> WorldAgentCallback {
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return WorldAgentCallback::default();
    }
    let mut callback = WorldAgentCallback::default();
    let pairs: Box<dyn Iterator<Item = (String, String)>> =
        if let Ok(url) = url::Url::parse(trimmed) {
            Box::new(
                url.query_pairs()
                    .into_owned()
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        } else {
            Box::new(
                url::form_urlencoded::parse(trimmed.as_bytes())
                    .into_owned()
                    .collect::<Vec<_>>()
                    .into_iter(),
            )
        };
    for (key, value) in pairs {
        match key.as_str() {
            "token" => callback.token = Some(value),
            "refresh_token" => callback.refresh_token = Some(value),
            "state" => callback.state = Some(value),
            "error" => callback.error = Some(value),
            "error_description" => callback.error_description = Some(value),
            _ => {}
        }
    }
    callback
}
```

- [ ] **Step 4: Add re-exports**

In `crates/puffer-provider-worldagent/src/lib.rs`, extend the `pub use` block:

```rust
pub use auth::{
    build_login_url, generate_client_state, parse_callback_input,
    WorldAgentCallback, WorldAgentLoginConfig,
    WORLDAGENT_AUTH_BASE_URL, WORLDAGENT_AUTH_URL_OVERRIDE_ENV,
    WORLDAGENT_CALLBACK_PATH, WORLDAGENT_CALLBACK_PORT,
    WORLDAGENT_DEFAULT_REDIRECT_URI,
};
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p puffer-provider-worldagent parse_callback_input
```

Expected: PASS (3 tests).

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-provider-worldagent/src
git commit -m "feat(provider-worldagent): parse callback URL into typed fields"
```

---

## Task 5: Implement `decode_jwt_profile`

**Files:**
- Modify: `crates/puffer-provider-worldagent/src/auth.rs`
- Modify: `crates/puffer-provider-worldagent/src/lib.rs`

- [ ] **Step 1: Write the failing test**

Append to the test module in `auth.rs`:

```rust
    #[test]
    fn decode_jwt_profile_reads_sub_email_name() {
        let payload = serde_json::json!({
            "sub": "user_01ABC",
            "email": "dev@example.com",
            "name": "Dev User",
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{encoded}.sig");
        let profile = decode_jwt_profile(&token);
        assert_eq!(profile.sub.as_deref(), Some("user_01ABC"));
        assert_eq!(profile.email.as_deref(), Some("dev@example.com"));
        assert_eq!(profile.name.as_deref(), Some("Dev User"));
    }

    #[test]
    fn decode_jwt_profile_handles_malformed_token() {
        let profile = decode_jwt_profile("not-a-jwt");
        assert!(profile.sub.is_none());
        assert!(profile.email.is_none());
        assert!(profile.name.is_none());
    }
```

- [ ] **Step 2: Run tests to verify they fail**

```bash
cargo test -p puffer-provider-worldagent decode_jwt_profile
```

Expected: FAIL — function does not exist.

- [ ] **Step 3: Implement**

Append below `parse_callback_input` (before the test module):

```rust
/// Decoded JWT profile fields, best-effort.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WorldAgentJwtProfile {
    pub sub: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Decodes `sub` / `email` / `name` from the access token JWT
/// payload. Any decode/parse failure yields an empty profile.
pub fn decode_jwt_profile(access_token: &str) -> WorldAgentJwtProfile {
    let Some(payload_b64) = access_token.split('.').nth(1) else {
        return WorldAgentJwtProfile::default();
    };
    let Ok(payload_bytes) = URL_SAFE_NO_PAD.decode(payload_b64.as_bytes()) else {
        return WorldAgentJwtProfile::default();
    };
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(&payload_bytes) else {
        return WorldAgentJwtProfile::default();
    };
    WorldAgentJwtProfile {
        sub: value
            .get("sub")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        email: value
            .get("email")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
        name: value
            .get("name")
            .and_then(serde_json::Value::as_str)
            .map(ToString::to_string),
    }
}
```

- [ ] **Step 4: Add re-exports**

Extend the `pub use` block in `lib.rs`:

```rust
pub use auth::{
    build_login_url, decode_jwt_profile, generate_client_state,
    parse_callback_input, WorldAgentCallback, WorldAgentJwtProfile,
    WorldAgentLoginConfig, WORLDAGENT_AUTH_BASE_URL,
    WORLDAGENT_AUTH_URL_OVERRIDE_ENV, WORLDAGENT_CALLBACK_PATH,
    WORLDAGENT_CALLBACK_PORT, WORLDAGENT_DEFAULT_REDIRECT_URI,
};
```

- [ ] **Step 5: Run tests**

```bash
cargo test -p puffer-provider-worldagent decode_jwt_profile
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-provider-worldagent/src
git commit -m "feat(provider-worldagent): decode JWT profile (sub/email/name)"
```

---

## Task 6: Implement `WorldAgentOAuthCredentials` + `refresh_oauth_token` + `exchange_jwt_for_api_key` stub

**Files:**
- Modify: `crates/puffer-provider-worldagent/src/auth.rs`
- Modify: `crates/puffer-provider-worldagent/src/lib.rs`

- [ ] **Step 1: Write the failing test (for the stub)**

Append to the test module:

```rust
    #[test]
    fn exchange_jwt_for_api_key_is_a_placeholder() {
        let result = exchange_jwt_for_api_key("any.access.token");
        assert!(result.is_err());
        let err = format!("{}", result.unwrap_err());
        assert!(err.contains("not yet implemented"));
    }
```

(`refresh_oauth_token` hits the network; we don't write a network-level test in this crate. Coverage of the wire shape lives in the daemon integration smoke test in Task 11.)

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-provider-worldagent exchange_jwt_for_api_key
```

Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Implement types + functions**

Append below `decode_jwt_profile`:

```rust
/// Persisted Auth Station credentials for the worldagent provider.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WorldAgentOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    pub sub: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Exchanges a stored refresh token for a new access token via
/// `POST <auth>/token/refresh`. Preserves the existing
/// refresh_token/profile fields when the upstream response does not
/// return them (Auth Station does not rotate refresh tokens, and
/// `/token/refresh` returns only `{ "token": ... }`).
pub fn refresh_oauth_token(
    refresh_token: &str,
    auth_base_url: Option<&str>,
) -> Result<WorldAgentOAuthCredentials> {
    let base = auth_base_url
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .unwrap_or_else(|| {
            std::env::var(WORLDAGENT_AUTH_URL_OVERRIDE_ENV)
                .ok()
                .filter(|value| !value.trim().is_empty())
                .unwrap_or_else(|| WORLDAGENT_AUTH_BASE_URL.to_string())
        });
    let url = format!("{}/token/refresh", base.trim_end_matches('/'));
    let response = Client::new()
        .post(&url)
        .json(&serde_json::json!({ "refresh_token": refresh_token }))
        .send()
        .context("failed to send worldagent refresh request")?;
    let status = response.status();
    let payload: RefreshResponse = response
        .json()
        .context("failed to parse worldagent refresh response")?;
    if !status.is_success() {
        return Err(anyhow!(
            "worldagent token refresh failed with status {status}: {}",
            payload.error.unwrap_or_default()
        ));
    }
    let access_token = payload
        .token
        .ok_or_else(|| anyhow!("worldagent refresh response missing token"))?;
    let profile = decode_jwt_profile(&access_token);
    Ok(WorldAgentOAuthCredentials {
        access_token,
        refresh_token: refresh_token.to_string(),
        expires_at_ms: now_ms() + 24 * 3600 * 1000,
        sub: profile.sub,
        email: profile.email,
        name: profile.name,
    })
}

/// Exchanges an Auth Station JWT for an inference API key.
///
/// **TODO (waiting on worldrouter backend):** the endpoint and
/// request shape are not yet finalized. Once defined, this function
/// will `POST <worldrouter>/api/v1/keys/exchange` (or whatever the
/// backend picks) with `Authorization: Bearer <access_token>` and
/// return the `api_key` string. The login handler will then upgrade
/// the stored credential to an `ApiKey { key }` variant.
pub fn exchange_jwt_for_api_key(_access_token: &str) -> Result<String> {
    Err(anyhow!(
        "worldagent JWT-to-api-key exchange is not yet implemented; \
         paste your WorldRouter API key for now"
    ))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

#[derive(Debug, Deserialize)]
struct RefreshResponse {
    #[serde(default)]
    token: Option<String>,
    #[serde(default)]
    error: Option<String>,
}
```

- [ ] **Step 4: Extend re-exports in lib.rs**

Final `pub use` block:

```rust
pub use auth::{
    build_login_url, decode_jwt_profile, exchange_jwt_for_api_key,
    generate_client_state, parse_callback_input, refresh_oauth_token,
    WorldAgentCallback, WorldAgentJwtProfile, WorldAgentLoginConfig,
    WorldAgentOAuthCredentials, WORLDAGENT_AUTH_BASE_URL,
    WORLDAGENT_AUTH_URL_OVERRIDE_ENV, WORLDAGENT_CALLBACK_PATH,
    WORLDAGENT_CALLBACK_PORT, WORLDAGENT_DEFAULT_REDIRECT_URI,
};
```

- [ ] **Step 5: Run all crate tests**

```bash
cargo test -p puffer-provider-worldagent
```

Expected: PASS (build_login_url, parse_callback_input x3, decode_jwt_profile x2, exchange_jwt_for_api_key — at least 7 tests pass).

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-provider-worldagent/src
git commit -m "feat(provider-worldagent): add credentials + refresh + exchange stub"
```

---

## Task 7: Add `bind_localhost_port` to `authflow.rs`

**Files:**
- Modify: `crates/puffer-cli/src/authflow.rs`

- [ ] **Step 1: Write the failing test**

Append to the existing `tests` module in `crates/puffer-cli/src/authflow.rs`:

```rust
    #[test]
    fn bind_localhost_port_uses_requested_port() {
        // Find a free port by binding 0, then drop and rebind on it.
        let probe = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = probe.local_addr().unwrap().port();
        drop(probe);
        let listener = CallbackListener::bind_localhost_port("/callback", port)
            .expect("bind_localhost_port succeeds on a free port");
        let redirect_uri = listener.redirect_uri();
        assert_eq!(
            redirect_uri,
            format!("http://127.0.0.1:{port}/callback")
        );
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-cli authflow::tests::bind_localhost_port
```

Expected: FAIL — method does not exist.

- [ ] **Step 3: Implement**

In `crates/puffer-cli/src/authflow.rs`, inside `impl CallbackListener`, right after `bind_localhost`:

```rust
    /// Binds a fixed loopback port. Used for redirect URIs that must
    /// match an Auth Station allow-list entry exactly (such as the
    /// worldagent provider). Returns an error if the port is in use.
    pub(crate) fn bind_localhost_port(path: &str, port: u16) -> Result<Self> {
        let listener = TcpListener::bind(("127.0.0.1", port)).with_context(|| {
            format!("failed to bind callback listener on 127.0.0.1:{port} for {path}")
        })?;
        listener.set_nonblocking(true)?;
        Ok(Self {
            listener,
            host: "127.0.0.1".to_string(),
            port,
            expected_path: path.to_string(),
            redirect_uri: format!("http://127.0.0.1:{port}{path}"),
        })
    }
```

- [ ] **Step 4: Run test**

```bash
cargo test -p puffer-cli authflow::tests::bind_localhost_port
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/puffer-cli/src/authflow.rs
git commit -m "feat(cli/authflow): add bind_localhost_port for fixed-port callbacks"
```

---

## Task 8: Wire `OauthFamily::WorldAgent` into `auth_provider.rs`

**Files:**
- Modify: `crates/puffer-cli/Cargo.toml`
- Modify: `crates/puffer-cli/src/auth_provider.rs`

- [ ] **Step 1: Add `puffer-provider-worldagent` as a dep of puffer-cli**

In `crates/puffer-cli/Cargo.toml`, find the `[dependencies]` block where the other `puffer-provider-*` entries live and add (alphabetically near `puffer-provider-openai`):

```toml
puffer-provider-worldagent = { path = "../puffer-provider-worldagent" }
```

- [ ] **Step 2: Write the failing test**

Append to the `tests` module in `crates/puffer-cli/src/auth_provider.rs`:

```rust
    #[test]
    fn oauth_family_uses_explicit_oauth_family_field() {
        let mut providers = ProviderRegistry::new();
        let mut descriptor = provider(
            "worldagent",
            "openai-completions",
            vec![AuthMode::OAuth, AuthMode::ApiKey],
        );
        descriptor.oauth_family = Some("worldagent".to_string());
        providers.register(descriptor);
        assert_eq!(
            oauth_family_for_provider(&providers, "worldagent"),
            Some(OauthFamily::WorldAgent)
        );
    }

    #[test]
    fn oauth_family_falls_back_to_default_api_when_field_unset() {
        let mut providers = ProviderRegistry::new();
        providers.register(provider(
            "custom-openai",
            "openai-completions",
            vec![AuthMode::OAuth],
        ));
        assert_eq!(
            oauth_family_for_provider(&providers, "custom-openai"),
            Some(OauthFamily::OpenAi)
        );
    }
```

Also update the existing `provider` test helper at the bottom of `auth_provider.rs` to set `oauth_family: None,` in the `ProviderDescriptor` literal. (Without this, the helper won't compile after Task 1 added the field.)

```rust
            chat_completions_path: None,
            oauth_family: None,
```

- [ ] **Step 3: Run test to verify it fails**

```bash
cargo test -p puffer-cli auth_provider::tests::oauth_family_uses_explicit
```

Expected: FAIL — `OauthFamily::WorldAgent` does not exist.

- [ ] **Step 4: Add the enum variant and dispatch logic**

In `crates/puffer-cli/src/auth_provider.rs`:

1. Extend `OauthFamily`:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum OauthFamily {
    Anthropic,
    OpenAi,
    WorldAgent,
}
```

2. Replace the body of `oauth_family_for_provider`:

```rust
pub(crate) fn oauth_family_for_provider(
    providers: &ProviderRegistry,
    provider_id: &str,
) -> Option<OauthFamily> {
    let provider = providers.provider(provider_id)?;
    if !provider.auth_modes.contains(&AuthMode::OAuth) {
        return None;
    }
    if let Some(family) = provider.oauth_family.as_deref() {
        return match family {
            "openai" => Some(OauthFamily::OpenAi),
            "anthropic" => Some(OauthFamily::Anthropic),
            "worldagent" => Some(OauthFamily::WorldAgent),
            _ => None,
        };
    }
    match provider.default_api.as_str() {
        "openai-responses"
        | "openai-completions"
        | "azure-openai-responses"
        | "openai-codex-responses" => Some(OauthFamily::OpenAi),
        "anthropic-messages" => Some(OauthFamily::Anthropic),
        _ => None,
    }
}
```

3. Add a `WorldAgent` arm to both `oauth_start_bundle_for_provider` and `oauth_login_bundle_for_provider`. Insert after the `Anthropic` arm in each:

```rust
        Some(OauthFamily::WorldAgent) => {
            let mut config = puffer_provider_worldagent::WorldAgentLoginConfig::default();
            // for oauth_login_bundle_for_provider only: override redirect_uri.
            // For oauth_start_bundle_for_provider, leave the default (the
            // fixed loopback URI baked into the crate).
            // …see step 5 for the exact code.
            Ok(OauthStartBundle {
                authorization_url: puffer_provider_worldagent::build_login_url(&config),
                automatic_authorization_url: None,
                verifier: String::new(),
                state: config.client_state,
                redirect_uri: config.redirect_uri,
                manual_redirect_uri: None,
            })
        }
```

- [ ] **Step 5: Apply the exact arms**

For `oauth_start_bundle_for_provider` (no explicit redirect_uri):

```rust
        Some(OauthFamily::WorldAgent) => {
            let config = puffer_provider_worldagent::WorldAgentLoginConfig::default();
            Ok(OauthStartBundle {
                authorization_url: puffer_provider_worldagent::build_login_url(&config),
                automatic_authorization_url: None,
                verifier: String::new(),
                state: config.client_state,
                redirect_uri: config.redirect_uri,
                manual_redirect_uri: None,
            })
        }
```

For `oauth_login_bundle_for_provider` (caller provides the bound redirect_uri):

```rust
        Some(OauthFamily::WorldAgent) => {
            let config = puffer_provider_worldagent::WorldAgentLoginConfig {
                redirect_uri: redirect_uri.to_string(),
                ..puffer_provider_worldagent::WorldAgentLoginConfig::default()
            };
            Ok(OauthStartBundle {
                authorization_url: puffer_provider_worldagent::build_login_url(&config),
                automatic_authorization_url: None,
                verifier: String::new(),
                state: config.client_state,
                redirect_uri: config.redirect_uri,
                manual_redirect_uri: None,
            })
        }
```

- [ ] **Step 6: Run tests**

```bash
cargo test -p puffer-cli auth_provider::tests
```

Expected: PASS (both new tests + the existing two unchanged).

- [ ] **Step 7: Commit**

```bash
git add crates/puffer-cli/Cargo.toml crates/puffer-cli/src/auth_provider.rs
git commit -m "feat(cli/auth_provider): dispatch worldagent OAuth family"
```

---

## Task 9: Add `to_registry_oauth_credential_worldagent` helper

**Files:**
- Modify: `crates/puffer-cli/src/auth_credentials.rs`

- [ ] **Step 1: Write the failing test**

Append (or create) a `#[cfg(test)] mod tests` block at the bottom of `auth_credentials.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use puffer_provider_worldagent::WorldAgentOAuthCredentials;

    #[test]
    fn worldagent_credential_maps_email_and_account_id() {
        let credential = WorldAgentOAuthCredentials {
            access_token: "acc".to_string(),
            refresh_token: "ref".to_string(),
            expires_at_ms: 42,
            sub: Some("user_01".to_string()),
            email: Some("dev@example.com".to_string()),
            name: Some("Dev".to_string()),
        };
        let stored = to_registry_oauth_credential_worldagent(credential);
        assert_eq!(stored.access_token, "acc");
        assert_eq!(stored.refresh_token, "ref");
        assert_eq!(stored.expires_at_ms, 42);
        assert_eq!(stored.account_id.as_deref(), Some("user_01"));
        assert_eq!(stored.email.as_deref(), Some("dev@example.com"));
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-cli auth_credentials::tests::worldagent_credential
```

Expected: FAIL — function doesn't exist.

- [ ] **Step 3: Implement**

Append (above the test module) in `crates/puffer-cli/src/auth_credentials.rs`:

```rust
/// Converts worldagent OAuth credentials into the registry storage shape.
/// The Auth Station `sub` claim is stored as `account_id` so the
/// existing AuthStore reuse path (organization_id, plan_type, etc.)
/// stays untouched. `name` is intentionally not persisted yet — the
/// existing `OAuthCredential` shape has no slot for it; if the UI
/// needs the display name later, we can either reuse `email` or
/// extend the struct.
pub(crate) fn to_registry_oauth_credential_worldagent(
    credential: puffer_provider_worldagent::WorldAgentOAuthCredentials,
) -> puffer_provider_registry::OAuthCredential {
    puffer_provider_registry::OAuthCredential {
        access_token: credential.access_token,
        refresh_token: credential.refresh_token,
        expires_at_ms: credential.expires_at_ms,
        account_id: credential.sub,
        organization_id: None,
        email: credential.email,
        plan_type: None,
        rate_limit_tier: None,
        scopes: Vec::new(),
        organization_name: None,
        organization_role: None,
        workspace_role: None,
    }
}
```

- [ ] **Step 4: Run test**

```bash
cargo test -p puffer-cli auth_credentials::tests::worldagent_credential
```

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/puffer-cli/src/auth_credentials.rs
git commit -m "feat(cli/auth_credentials): map worldagent credential into registry shape"
```

---

## Task 10: Wire worldagent into `run_login_flow` (CLI path)

**Files:**
- Modify: `crates/puffer-cli/src/main.rs`

- [ ] **Step 1: Add imports**

Near the existing `use puffer_provider_openai::{…}` and `use puffer_transport_anthropic::{…}` blocks, add:

```rust
use puffer_provider_worldagent::{
    decode_jwt_profile as decode_worldagent_jwt_profile,
    parse_callback_input as parse_worldagent_callback_input,
    WorldAgentOAuthCredentials, WORLDAGENT_CALLBACK_PATH, WORLDAGENT_CALLBACK_PORT,
};
```

And in the `use crate::auth_credentials::{…}` block (around line 65), add:

```rust
use crate::auth_credentials::to_registry_oauth_credential_worldagent;
```

- [ ] **Step 2: Replace the localhost listener bind in `run_login_flow`**

Currently `run_login_flow` always calls
`authflow::CallbackListener::bind_localhost("/callback")`. Replace
the listener-creation block (around line 1017–1021) with:

```rust
    let callback_listener = if stdin || value.is_some() {
        None
    } else if matches!(
        oauth_family_for_provider(providers, provider),
        Some(OauthFamily::WorldAgent)
    ) {
        Some(authflow::CallbackListener::bind_localhost_port(
            WORLDAGENT_CALLBACK_PATH,
            WORLDAGENT_CALLBACK_PORT,
        )?)
    } else {
        Some(authflow::CallbackListener::bind_localhost("/callback")?)
    };
```

- [ ] **Step 3: Add the `WorldAgent` arm to the outer `match`**

Inside `run_login_flow`, after the `OauthFamily::Anthropic` arm and before the `None =>` bail, insert:

```rust
        Some(OauthFamily::WorldAgent) => {
            let parsed = parse_worldagent_callback_input(&input);
            if let Some(err) = parsed.error.as_deref() {
                let desc = parsed.error_description.as_deref().unwrap_or("");
                anyhow::bail!("worldagent login failed: {err} {desc}");
            }
            if parsed.state.as_deref() != Some(bundle.state.as_str()) {
                anyhow::bail!("oauth state mismatch for worldagent");
            }
            let access_token = parsed
                .token
                .ok_or_else(|| anyhow::anyhow!("worldagent callback missing token"))?;
            let refresh_token = parsed.refresh_token.unwrap_or_default();
            let profile = decode_worldagent_jwt_profile(&access_token);
            let credential = WorldAgentOAuthCredentials {
                access_token,
                refresh_token,
                expires_at_ms: now_ms_for_worldagent_credential(),
                sub: profile.sub,
                email: profile.email,
                name: profile.name,
            };
            auth_store.set_oauth(
                provider.to_string(),
                to_registry_oauth_credential_worldagent(credential),
            );
        }
```

Add this helper near the bottom of `main.rs` (sibling of `resolve_provider_id`):

```rust
fn now_ms_for_worldagent_credential() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64 + 24 * 3600 * 1000)
        .unwrap_or(24 * 3600 * 1000)
}
```

- [ ] **Step 4: Run cli build + lib tests**

```bash
cargo build -p puffer-cli
cargo test -p puffer-cli --lib
```

Expected: build + tests SUCCESS.

- [ ] **Step 5: Commit**

```bash
git add crates/puffer-cli/src/main.rs
git commit -m "feat(cli): handle worldagent OAuth in run_login_flow"
```

---

## Task 11: Wire worldagent into the desktop daemon `handle_login_with_oauth`

**Files:**
- Modify: `crates/puffer-cli/src/daemon.rs`

- [ ] **Step 1: Add imports**

Near the top of `daemon.rs`, in the `use puffer_provider_openai::{…}` import block (or wherever the OpenAI/Anthropic imports live), add:

```rust
use puffer_provider_worldagent::{
    decode_jwt_profile as decode_worldagent_jwt_profile,
    parse_callback_input as parse_worldagent_callback_input,
    WorldAgentOAuthCredentials, WORLDAGENT_CALLBACK_PATH, WORLDAGENT_CALLBACK_PORT,
};
```

In the `use crate::auth_credentials::{…}` block:

```rust
use crate::auth_credentials::to_registry_oauth_credential_worldagent;
```

- [ ] **Step 2: Branch the listener bind**

In `handle_login_with_oauth` (around line 1070), replace:

```rust
    let listener = crate::authflow::CallbackListener::bind_localhost("/callback")?;
```

with:

```rust
    let listener = if matches!(
        oauth_family_for_provider(&inputs.providers, &provider_id),
        Some(OauthFamily::WorldAgent)
    ) {
        crate::authflow::CallbackListener::bind_localhost_port(
            WORLDAGENT_CALLBACK_PATH,
            WORLDAGENT_CALLBACK_PORT,
        )?
    } else {
        crate::authflow::CallbackListener::bind_localhost("/callback")?
    };
```

- [ ] **Step 3: Add the `WorldAgent` arm to the `match`**

After the `OauthFamily::Anthropic` arm (around line 1101–1121) and before the `None =>` bail:

```rust
        Some(OauthFamily::WorldAgent) => {
            let parsed = parse_worldagent_callback_input(&callback);
            if let Some(err) = parsed.error.as_deref() {
                let desc = parsed.error_description.as_deref().unwrap_or("");
                anyhow::bail!("worldagent login failed: {err} {desc}");
            }
            if parsed.state.as_deref() != Some(bundle.state.as_str()) {
                anyhow::bail!("oauth state mismatch for worldagent");
            }
            let access_token = parsed
                .token
                .ok_or_else(|| anyhow::anyhow!("worldagent callback missing token"))?;
            let refresh_token = parsed.refresh_token.unwrap_or_default();
            let profile = decode_worldagent_jwt_profile(&access_token);
            let expires_at_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64 + 24 * 3600 * 1000)
                .unwrap_or(24 * 3600 * 1000);
            let credential = WorldAgentOAuthCredentials {
                access_token,
                refresh_token,
                expires_at_ms,
                sub: profile.sub,
                email: profile.email,
                name: profile.name,
            };
            set_stored_credential(
                &mut inputs.auth_store,
                provider_id.to_string(),
                StoredCredential::OAuth(to_registry_oauth_credential_worldagent(credential)),
            );
        }
```

- [ ] **Step 4: Build the cli**

```bash
cargo build -p puffer-cli
```

Expected: SUCCESS.

- [ ] **Step 5: Run all puffer-cli tests**

```bash
cargo test -p puffer-cli --lib
```

Expected: PASS.

- [ ] **Step 6: Commit**

```bash
git add crates/puffer-cli/src/daemon.rs
git commit -m "feat(cli/daemon): handle worldagent OAuth in handle_login_with_oauth"
```

---

## Task 12: Ship the `worldagent.yaml` provider resource

**Files:**
- Create: `resources/providers/worldagent.yaml`

- [ ] **Step 1: Write the failing test**

Append to the test module in `crates/puffer-resources/src/model.rs` (right next to `zhipu_yaml_parses_with_chat_completions_path_override`):

```rust
    /// Confirms the bundled `worldagent.yaml` parses as a
    /// `ProviderPack` and that the `oauth_family` field round-trips
    /// through `into_descriptor`. Without this end-to-end wiring the
    /// runtime would silently fall back to OpenAI OAuth.
    #[test]
    fn worldagent_yaml_parses_with_oauth_family() {
        let yaml = include_str!("../../../resources/providers/worldagent.yaml");
        let pack: ProviderPack = serde_yaml::from_str(yaml).expect("worldagent.yaml parses");
        assert_eq!(pack.id, "worldagent");
        assert_eq!(pack.oauth_family.as_deref(), Some("worldagent"));
        let descriptor = pack.into_descriptor();
        assert_eq!(descriptor.oauth_family.as_deref(), Some("worldagent"));
        assert!(descriptor.auth_modes.contains(&AuthMode::ApiKey));
        assert!(descriptor.auth_modes.contains(&AuthMode::OAuth));
    }
```

- [ ] **Step 2: Run test to verify it fails**

```bash
cargo test -p puffer-resources worldagent_yaml_parses_with_oauth_family
```

Expected: FAIL — file doesn't exist (`include_str!` won't compile).

- [ ] **Step 3: Create the yaml**

`resources/providers/worldagent.yaml`:

```yaml
id: worldagent
display_name: WorldAgent
base_url: https://inference-api.worldrouter.ai
default_api: openai-completions
oauth_family: worldagent
auth_modes:
  - api_key
  - oauth
discovery:
  path: /v1/models
  response: open_ai_models
  api: openai-completions
  context_window: 200000
  max_output_tokens: 8192
  supports_reasoning: true
models:
  - id: gpt-5
    display_name: GPT-5 (via WorldRouter)
    provider: worldagent
    api: openai-completions
    context_window: 200000
    max_output_tokens: 8192
    supports_reasoning: true
```

- [ ] **Step 4: Run test**

```bash
cargo test -p puffer-resources worldagent_yaml_parses_with_oauth_family
```

Expected: PASS.

- [ ] **Step 5: Build the cli end-to-end and run all tests**

```bash
cargo build -p puffer-cli
cargo test --workspace
```

Expected: SUCCESS / PASS. Note: tests that hit the network (Auth Station, OpenAI, Anthropic) are not added in this plan; the cited `cargo test --workspace` should remain green with no new network calls.

- [ ] **Step 6: Commit**

```bash
git add resources/providers/worldagent.yaml crates/puffer-resources/src/model.rs
git commit -m "feat(resources): ship worldagent provider yaml"
```

---

## Task 13: Add desktop visual entry for worldagent

**Files:**
- Modify: `apps/puffer-desktop/src/lib/providerVisuals.ts`

- [ ] **Step 1: Inspect existing entries**

```bash
grep -n "openai\|anthropic\|groq\|kimi\|providerVisual\|export" apps/puffer-desktop/src/lib/providerVisuals.ts | head -40
```

Read the file once to understand the registration shape (icon path, accent color, fallback handling).

- [ ] **Step 2: Add a `worldagent` entry**

Match the shape used by existing providers. If entries are keyed by provider id in a record, insert (alphabetically):

```typescript
  worldagent: {
    icon: "/icons/providers/worldagent.svg", // or use the existing generic fallback
    accent: "#1f6feb",
    displayName: "WorldAgent",
  },
```

If your repo doesn't ship a `worldagent.svg`, **use the existing fallback icon** rather than fabricating an asset. Pick whatever the file already does for unknown providers (likely a default monogram or a tinted placeholder). Do not create new SVG/PNG files in this task — the design says "designer can replace later".

- [ ] **Step 3: Smoke-build the frontend**

```bash
cd apps/puffer-desktop
pnpm install --frozen-lockfile
pnpm check
```

Expected: `svelte-check` reports zero errors related to your edit. (Pre-existing warnings unrelated to worldagent are fine.)

- [ ] **Step 4: Commit**

```bash
git add apps/puffer-desktop/src/lib/providerVisuals.ts
git commit -m "feat(desktop): register worldagent provider visuals"
```

---

## Task 14: Write per-component update specs

**Files:**
- Create: `specs/puffer-provider-worldagent/00.md`
- Create: `specs/puffer-provider-registry/06.md`
- Create: `specs/puffer-cli/<next>.md` (use the next free `NN.md`, list the directory first)
- Create: `specs/puffer-resources/<next>.md`
- Create: `specs/puffer-desktop/<next>.md`

- [ ] **Step 1: Determine the next free numeric prefix for each component**

```bash
ls specs/puffer-cli | tail
ls specs/puffer-resources | tail
ls specs/puffer-desktop | tail
```

Use the next unused two-digit prefix per AGENTS.md ("do not overwrite prior numbered specs").

- [ ] **Step 2: Write each spec**

Each file is ≤ 60 lines, follows the existing terse style (see `specs/puffer-provider-openai/01.md`):

`specs/puffer-provider-worldagent/00.md`:

```markdown
# WorldAgent Provider Crate

## Summary
- New crate `puffer-provider-worldagent` owning the Auth Station login flow for the `worldagent` provider.
- Auth Station returns final `token` and `refresh_token` directly in the callback URL; this crate models that flow (no PKCE, no code exchange).

## Surface
- `build_login_url(&WorldAgentLoginConfig) -> String`
- `parse_callback_input(&str) -> WorldAgentCallback`
- `decode_jwt_profile(&str) -> WorldAgentJwtProfile`
- `refresh_oauth_token(&str, Option<&str>) -> Result<WorldAgentOAuthCredentials>`
- `exchange_jwt_for_api_key(&str) -> Result<String>` — TODO stub, waits on worldrouter backend

## Configuration
- Default Auth Station URL: `https://auth-worldrouter.vercel.app` (Sandbox).
- Override via env var `PUFFER_WORLDAGENT_AUTH_URL`.
- Fixed loopback redirect: `http://127.0.0.1:1456/callback`.

## Compatibility
- The fixed redirect URI must be allow-listed in Auth Station `ALLOWED_REDIRECT_ORIGINS` on both Sandbox and Production.
- JWT-to-api-key exchange is intentionally a stub; until the backend endpoint lands, the OAuth path does not yield an inference-usable credential. Users still must paste an API key.
```

`specs/puffer-provider-registry/06.md`:

```markdown
# Optional `oauth_family` on ProviderDescriptor

## Summary
- `ProviderDescriptor` gains `oauth_family: Option<String>`.
- When `None`, callers infer the OAuth family from `default_api` (no behavior change for existing yaml).
- When `Some`, callers dispatch directly to the named family. Known values today: `openai`, `anthropic`, `worldagent`.

## Compatibility
- Default value preserves every existing provider yaml.
- `ProviderPack` (in `puffer-resources`) mirrors the field and threads it through `into_descriptor`.
```

`specs/puffer-cli/<next>.md` (worldagent OAuth dispatch):

```markdown
# WorldAgent OAuth Dispatch

## Summary
- `OauthFamily` grows a `WorldAgent` variant.
- `oauth_family_for_provider` prefers `descriptor.oauth_family` when set, otherwise falls back to `default_api`.
- `oauth_login_bundle_for_provider` builds the bundle from `WorldAgentLoginConfig`; `verifier` is empty (no PKCE), `automatic_authorization_url` is `None`.
- `handle_login_with_oauth` (daemon) and `run_login_flow` (cli) both gain a `WorldAgent` arm: parse callback, verify `state`, decode JWT for `sub`/`email`/`name`, store as `StoredCredential::OAuth`.
- `CallbackListener::bind_localhost_port` lets the daemon bind the fixed `127.0.0.1:1456` Auth-Station-whitelist port.

## Compatibility
- Existing OpenAI / Anthropic flows are unchanged (`oauth_family` unset → falls back to `default_api` map).
- `WorldAgentOAuthCredentials.name` is not yet persisted (no slot on `OAuthCredential`); only `sub`/`email` survive into the registry shape.
```

`specs/puffer-resources/<next>.md`:

```markdown
# WorldAgent Provider Yaml

## Summary
- Bundled `resources/providers/worldagent.yaml` adds the `worldagent` provider entry.
- `default_api: openai-completions` (inference goes through the existing OpenAI chat-completions transport).
- `oauth_family: worldagent` opts into the new login dispatch.
- `auth_modes: [api_key, oauth]` exposes both LoginView paths.
- Model catalog is seeded minimally; `/v1/models` discovery populates the rest at runtime.

## Compatibility
- Pure addition; no existing yaml is modified.
```

`specs/puffer-desktop/<next>.md`:

```markdown
# WorldAgent Visuals

## Summary
- `providerVisuals.ts` registers a `worldagent` entry (display name + accent).
- No bespoke icon ships in this change; the fallback icon is reused until design provides one.

## Compatibility
- LoginView's generic OAuth / api_key surface needs no component change.
```

- [ ] **Step 3: Commit**

```bash
git add specs/puffer-provider-worldagent specs/puffer-provider-registry \
        specs/puffer-cli specs/puffer-resources specs/puffer-desktop
git commit -m "docs(specs): document worldagent provider integration"
```

---

## Task 15: Final verification

- [ ] **Step 1: Workspace test**

```bash
cargo test --workspace
```

Expected: all tests green, including the new worldagent tests and the resource yaml parse test.

- [ ] **Step 2: Workspace build**

```bash
cargo build --workspace
```

Expected: SUCCESS.

- [ ] **Step 3: Desktop typecheck**

```bash
cd apps/puffer-desktop
pnpm check
```

Expected: no new errors. (Pre-existing warnings are out of scope.)

- [ ] **Step 4: Manual smoke (optional, requires daemon)**

If a daemon is reachable and `http://127.0.0.1:1456/callback` is allow-listed:

1. Start the daemon: `cargo run -p puffer-cli -- daemon`
2. Open the desktop app, navigate to Login screen.
3. Find the **WorldAgent** card.
4. Click "Connect with OAuth".
5. Verify the browser opens `https://auth-worldrouter.vercel.app/login?redirect_uri=http://127.0.0.1:1456/callback&client_state=...`
6. Sign in. Browser should redirect to a success page.
7. Confirm `~/.config/puffer/auth.json` (or equivalent platform path) shows a `worldagent` entry with `kind: oauth`.
8. Paste a real WorldRouter API key in the WorldAgent card too — verify the stored credential flips to `kind: api_key`.

If the manual smoke fails because the redirect URI is not allow-listed yet, **that is the expected failure mode** until the auth maintainer adds `http://127.0.0.1:1456/callback` to `ALLOWED_REDIRECT_ORIGINS` on Sandbox + Production. Note the failure mode in the PR description.

- [ ] **Step 5: Final commit (only if any clean-up edits were needed)**

```bash
git status
# If no further changes, skip. Otherwise:
git add -p
git commit -m "chore(worldagent): verification follow-ups"
```

---

## Self-Review

**Spec coverage:**
- §3 architecture diagram → Tasks 7, 8, 10, 11
- §4 provider yaml → Task 12
- §5 ProviderDescriptor field → Task 1
- §6 new crate surface → Tasks 2–6
- §7 daemon login dispatch + `bind_localhost_port` → Tasks 7, 11
- §8 TODO JWT→api_key + UI banner — Task 6 (stub) + Task 13 (visuals). The visible banner copy described in §8 of the spec is currently surfaced by `statusMessage = "Connected to worldagent."` plus the user manually pasting an api_key when ready. Adding a worldagent-specific banner above the OAuth button is **deferred** (it requires LoginView changes, which the spec explicitly said were "no LoginView component change needed"). If the user wants a banner, raise it during plan review.
- §9 desktop UI → Task 13
- §10 tests → Tasks 1, 3, 4, 5, 6, 7, 8, 9, 12 each ship the tests they own
- §11 per-component specs → Task 14
- §12 pre-shipping checklist → noted in Task 15 step 4

**Placeholder scan:** all code blocks contain literal source; the only "TODO" lives in `exchange_jwt_for_api_key`, which is intentional and tested (asserts the function `bail!`s with "not yet implemented"). The `<next>.md` filename placeholder in Task 14 is resolved at step 1 by listing the directory.

**Type consistency:**
- `WorldAgentLoginConfig` / `WorldAgentCallback` / `WorldAgentJwtProfile` / `WorldAgentOAuthCredentials` all defined in Task 3/4/5/6 and consumed in Tasks 8/10/11 by the same names.
- `OauthFamily::WorldAgent` defined in Task 8 and consumed in Tasks 10/11.
- `to_registry_oauth_credential_worldagent` defined in Task 9, consumed in Tasks 10/11.
- `bind_localhost_port` defined in Task 7, consumed in Tasks 10/11.
- `WORLDAGENT_CALLBACK_PATH` / `WORLDAGENT_CALLBACK_PORT` defined in Task 3, consumed in Tasks 10/11.

No gaps.
