# worldagent Provider — Design

Date: 2026-05-20
Status: Draft (awaiting user approval)
Owner: sean

## 1. Motivation

WorldRouter exposes an OpenAI-compatible inference API at
`https://inference-api.worldrouter.ai`. Puffer needs a first-class
provider entry so users can:

1. Paste a WorldRouter API key (same UX as existing OpenAI-compatible
   relays — kimi-openai, groq, openrouter, etc.).
2. Click "Connect with OAuth" and authorize via Auth Station
   (`https://auth.worldrouter.ai` / Sandbox
   `https://auth-worldrouter.vercel.app`), opening the auth website in
   the default browser and capturing a callback locally.

Long-term framing (from user): worldagent is a **brand entry point**.
The provider role is the minimum-impact form for the current Puffer
flow (provider + model + routing + auth all reuse existing plumbing).
Future iterations will let the OAuth session resolve to either a
`claw` plan or a WorldRouter API key. The current scope keeps both
paths open: OAuth captures the JWT and stores it; API-key input
stores a key. The JWT-to-api-key exchange endpoint is **not yet
defined backend-side** and is a clearly marked TODO in code.

## 2. Non-Goals

- Implementing the JWT → inference API-key exchange. Backend is not
  ready. We store the JWT as an OAuth credential placeholder; the
  inference path requires a manually pasted API key for now.
- Replacing existing OpenAI provider crate functionality.
- Reworking LoginView UI. The component is already generic over
  `authModes`.
- Renaming the provider id (`worldagent` vs `worldclaw`). Pick one id
  now and keep it stable; display name can change later via yaml.

## 3. High-level architecture

```
┌─────────────────────┐    OAuth button       ┌──────────────────┐
│  puffer-desktop     │  ───────────────────▶ │  daemon          │
│  (Svelte LoginView) │   login_with_oauth    │  (puffer-cli)    │
└─────────────────────┘                       └────────┬─────────┘
                                                       │
                                                       │ dispatch by oauth_family
                                                       ▼
                          ┌────────────────────────────────────────┐
                          │ puffer-provider-worldagent             │
                          │  build_login_url + parse_callback +    │
                          │  decode_jwt + refresh_token            │
                          └────────────┬───────────────────────────┘
                                       │ open browser
                                       ▼
                          ┌────────────────────────────────────────┐
                          │ Auth Station                           │
                          │  /login → 302 to                       │
                          │  http://127.0.0.1:1456/callback?token= │
                          └────────────────────────────────────────┘
                                       │ daemon waits on CallbackListener
                                       ▼
                          ┌────────────────────────────────────────┐
                          │ AuthStore (OAuth credential)           │
                          │  access_token / refresh_token /        │
                          │  email / sub from JWT                  │
                          └────────────────────────────────────────┘
```

API-key path is unchanged: LoginView submits api_key → existing
`store_api_key` plumbing → `StoredCredential::ApiKey` in AuthStore.

## 4. Provider yaml

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
  # Seed list — actual catalog comes from /v1/models discovery.
  - id: gpt-5
    display_name: GPT-5 (via WorldRouter)
    provider: worldagent
    api: openai-completions
    context_window: 200000
    max_output_tokens: 8192
    supports_reasoning: true
```

Notes:
- `default_api: openai-completions` → reuses the existing OpenAI
  Chat-Completions transport (Bearer api_key, `/v1/chat/completions`).
- `oauth_family: worldagent` is a new field (§5). When unset, the
  registry falls back to the existing API-family inference.
- The model seed list is intentionally minimal; `discovery` will
  populate the rest at runtime against `/v1/models`.

## 5. ProviderDescriptor change

`crates/puffer-provider-registry/src/model.rs`:

```rust
pub struct ProviderDescriptor {
    // ...existing fields...
    /// Optional explicit OAuth family. When `None`, callers infer the
    /// family from `default_api` (existing behavior — preserves
    /// every yaml that did not opt in). When `Some`, callers use the
    /// named family directly. This is the seam that lets a single
    /// provider with `default_api: openai-completions` plug into a
    /// non-OpenAI OAuth flow.
    #[serde(default)]
    pub oauth_family: Option<String>,
}
```

`auth_provider.rs::oauth_family_for_provider` is updated to:

1. Read `descriptor.oauth_family` first; map known strings to enum:
   - `"openai"` → `OauthFamily::OpenAi`
   - `"anthropic"` → `OauthFamily::Anthropic`
   - `"worldagent"` → `OauthFamily::WorldAgent` (new)
2. If unset, fall back to the existing `default_api` switch (no
   behavior change for any existing yaml).

`OauthFamily` enum grows one variant: `WorldAgent`.

## 6. New crate: `puffer-provider-worldagent`

Layout:

```
crates/puffer-provider-worldagent/
├── Cargo.toml
└── src/
    ├── lib.rs    — public re-exports
    └── auth.rs   — login URL / callback parser / JWT decode / refresh
```

Public surface (`src/auth.rs`):

```rust
/// Default Auth Station base URL (Sandbox).
pub const WORLDAGENT_AUTH_BASE_URL: &str = "https://auth-worldrouter.vercel.app";

/// Env var that overrides the Auth Station base URL.
pub const WORLDAGENT_AUTH_URL_OVERRIDE_ENV: &str = "PUFFER_WORLDAGENT_AUTH_URL";

/// Fixed loopback callback used by Puffer desktop. The auth team
/// must allow-list this redirect URI on both Sandbox and Production
/// `ALLOWED_REDIRECT_ORIGINS`.
pub const WORLDAGENT_CALLBACK_PATH: &str = "/callback";
pub const WORLDAGENT_CALLBACK_PORT: u16 = 1456;
pub const WORLDAGENT_DEFAULT_REDIRECT_URI: &str =
    "http://127.0.0.1:1456/callback";

/// Persisted Auth Station credentials for the worldagent provider.
pub struct WorldAgentOAuthCredentials {
    pub access_token: String,
    pub refresh_token: String,
    pub expires_at_ms: u64,
    pub sub: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Parameters required to build the Auth Station login URL.
pub struct WorldAgentLoginConfig {
    pub auth_base_url: String,
    pub redirect_uri: String,
    pub client_state: String,
}

impl Default for WorldAgentLoginConfig { /* env override + defaults */ }

/// Generate an opaque random client_state.
pub fn generate_client_state() -> String;

/// Build the GET URL for `<auth>/login?redirect_uri=&client_state=`.
pub fn build_login_url(config: &WorldAgentLoginConfig) -> String;

/// Parsed callback fields. Each field is `None` when the parameter
/// was absent from the callback URL.
pub struct WorldAgentCallback {
    pub token: Option<String>,
    pub refresh_token: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

/// Extract `token`, `refresh_token`, `state`, `error`,
/// `error_description` from a callback URL.
pub fn parse_callback_input(input: &str) -> WorldAgentCallback;

/// Decoded JWT profile fields, best-effort.
pub struct WorldAgentJwtProfile {
    pub sub: Option<String>,
    pub email: Option<String>,
    pub name: Option<String>,
}

/// Decode `sub`/`email`/`name` from the access token JWT payload
/// (best-effort; failures yield empty fields).
pub fn decode_jwt_profile(access_token: &str) -> WorldAgentJwtProfile;

/// Exchange a stored refresh token for a new access token via
/// `POST <auth>/token/refresh`.
pub fn refresh_oauth_token(
    refresh_token: &str,
    auth_base_url: Option<&str>,
) -> Result<WorldAgentOAuthCredentials>;
```

Auth Station's `/login` flow is **simpler than OAuth**: there is no
PKCE, no code, no token exchange. The callback already contains the
final tokens. The CSRF guard is `client_state ↔ state`. We honor it.

`auth.rs` stays well under the 1000-line limit (target ~250 lines
including tests).

## 7. Daemon login dispatch

`crates/puffer-cli/src/daemon.rs::handle_login_with_oauth` gains a
third arm:

```rust
Some(OauthFamily::WorldAgent) => {
    let parsed = parse_callback_input(&callback);
    if let Some(err) = parsed.error.as_deref() {
        let desc = parsed.error_description.as_deref().unwrap_or("");
        bail!("worldagent login failed: {err} {desc}");
    }
    if parsed.state.as_deref() != Some(bundle.state.as_str()) {
        bail!("oauth state mismatch for worldagent");
    }
    let token = parsed
        .token
        .ok_or_else(|| anyhow!("worldagent callback missing token"))?;
    let refresh = parsed.refresh_token.unwrap_or_default();
    let profile = decode_jwt_profile(&token);
    let credential = WorldAgentOAuthCredentials {
        access_token: token,
        refresh_token: refresh,
        expires_at_ms: now_ms() + 24 * 3600 * 1000, // matches Auth Station spec
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

`oauth_login_bundle_for_provider` (auth_provider.rs) likewise gains a
`WorldAgent` arm that builds the bundle from
`WorldAgentLoginConfig`. The bundle's `verifier` is unused for
worldagent — we set it to an empty string. `automatic_authorization_url`
is `None` (single URL, no manual fallback).

`puffer-cli/src/main.rs::run_login_flow` adds the matching arm so the
CLI path (`puffer auth login worldagent`) works the same way.

`puffer-cli/src/authflow.rs` is unchanged. The `CallbackListener::bind_localhost`
helper accepts the fixed port via a new optional binder
`bind_localhost_port(path, port)` (the existing helper stays the
default — we only branch when the caller asks for a fixed port).

## 8. TODO: JWT → api_key exchange

A clearly named module placeholder is added in
`puffer-provider-worldagent/src/lib.rs`:

```rust
/// TODO (waiting on worldrouter backend):
/// Exchange an Auth Station JWT for an inference API key.
/// Once the endpoint is finalized, this function POSTs the JWT to
/// `<worldrouter>/api/v1/keys/exchange` (or whatever the backend
/// settles on) and returns the api_key. The login handler will then
/// upgrade the stored credential from `OAuth(jwt)` to `ApiKey(...)`
/// (or store both).
pub fn exchange_jwt_for_api_key(
    _access_token: &str,
) -> Result<String> {
    anyhow::bail!(
        "worldagent JWT-to-api-key exchange is not yet implemented; \
         paste your WorldRouter API key for now."
    )
}
```

The `handle_login_with_oauth` arm calls this **eagerly** and, on the
expected `bail!`, stores the OAuth credential anyway and surfaces a
user-visible message in the SettingsSnapshot:
`"Logged in as <email>. Paste your WorldRouter API key to enable
inference (auto-exchange pending backend support)."` The next
SettingsSnapshot fetch refreshes the UI.

When backend ships the endpoint, only the body of
`exchange_jwt_for_api_key` and the login handler's branch change.

## 9. Desktop UI

LoginView already supports the API-key + OAuth dual layout. The only
desktop-side change is:

- `apps/puffer-desktop/src/lib/providerVisuals.ts` — register a
  `worldagent` entry (icon path + accent color). A simple text-based
  monogram icon is acceptable for v1; designer can replace later.
- A short banner above the OAuth button when the active provider is
  worldagent and only an OAuth credential exists (no api_key): "Auto
  api-key exchange is not yet enabled. Paste a WorldRouter API key
  to start running models."

No new Tauri commands. No new daemon RPCs beyond reusing
`login_with_oauth`.

## 10. Tests

- `puffer-provider-worldagent`:
  - `build_login_url_contains_redirect_uri_and_client_state`
  - `parse_callback_input_extracts_token_refresh_state`
  - `parse_callback_input_returns_error_when_present`
  - `decode_jwt_profile_reads_sub_email_name`
  - `default_config_honors_env_override`
- `puffer-provider-registry`:
  - `provider_descriptor_deserializes_oauth_family_field`
  - `oauth_family_field_defaults_to_none`
- `puffer-cli/auth_provider`:
  - `oauth_family_uses_explicit_field_when_set`
  - `oauth_family_falls_back_to_default_api_when_unset`
  - `oauth_family_recognizes_worldagent`
- `puffer-cli/daemon` (with `tokio::test`):
  - Smoke test for `handle_login_with_oauth` with a fake worldagent
    callback URL passed through the bundle path.

`cargo test --workspace` must stay green.

## 11. Resource provenance / file moves

No moves. New files:

- `resources/providers/worldagent.yaml`
- `crates/puffer-provider-worldagent/Cargo.toml`
- `crates/puffer-provider-worldagent/src/lib.rs`
- `crates/puffer-provider-worldagent/src/auth.rs`

New per-component spec files (per AGENTS.md convention):

- `specs/puffer-provider-worldagent/00.md` — crate overview
- `specs/puffer-provider-registry/06.md` — `oauth_family` field
- `specs/puffer-cli/<next>.md` — auth_provider dispatch + daemon arm
- `specs/puffer-resources/<next>.md` — worldagent.yaml entry
- `specs/puffer-desktop/<next>.md` — providerVisuals entry + banner

Each component spec is concise (≤ 60 lines) per existing style.

## 12. Pre-shipping checklist for the user (out of code)

- File a request with the auth maintainer to allow-list
  `http://127.0.0.1:1456/callback` on **both** Sandbox and
  Production `ALLOWED_REDIRECT_ORIGINS`.
- Confirm `aud=worldclaw` is the correct audience claim for the
  worldagent product (the current docs use `worldclaw`; if a
  separate audience is preferred for this product, surface it now).
- Confirm the final brand name (`worldagent` vs `worldclaw`) for the
  yaml `id`. If you want to switch later, the cost is one yaml
  rename plus a credentials migration step.

## 13. Future work (post-MVP)

- JWT → api_key exchange (waits on backend).
- Profile UI showing the authenticated email / org from the JWT.
- Refresh token rotation when access_token expires (one-line cron in
  daemon: call `refresh_oauth_token` and re-store the credential).
- "Switch account" button = `puffer auth logout worldagent` + repeat
  the OAuth flow.
- If/when the brand becomes the primary entry point: hoist the
  worldagent OAuth flow to the onboarding root, push the
  raw-OpenAI/Anthropic providers into an "Advanced" sub-screen.
