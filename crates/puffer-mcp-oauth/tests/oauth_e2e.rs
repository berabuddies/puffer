//! End-to-end OAuth flow tests against the in-tree stub server.
//!
//! Exercises the public surface of [`puffer_mcp_oauth::OAuthService`]:
//!
//! * `auth_code_flow_completes_against_stub`: silent flow drives
//!   discovery + DCR + browser-redirect-substitute + callback + token
//!   exchange end-to-end, then `resolve()` returns a usable AuthClient.
//! * `tokens_persist_across_runner_restarts`: write tokens with one
//!   service instance, drop it, build another against the same `token_dir`,
//!   verify resolve() succeeds without re-auth.
//! * `token_refresh_on_expiry_succeeds`: configure the stub for a 1s
//!   token TTL, sleep past expiry, resolve(), verify the refresh path
//!   ran and the new token is in use.
//! * `expired_refresh_token_returns_oauth_required`: configure the stub
//!   to reject refresh, simulate an expired access token, resolve()
//!   surfaces `OAuthRequired`.

#[path = "oauth_stub_server.rs"]
mod oauth_stub_server;

use std::time::Duration;

use puffer_mcp_oauth::{OAuthConfig, OAuthError, OAuthService};
use tempfile::TempDir;

use oauth_stub_server::{spawn_oauth_stub, OAuthStubConfig};

fn config_for(server_url: &str, token_dir: std::path::PathBuf) -> OAuthConfig {
    OAuthConfig {
        server_id: "stub-server".into(),
        server_url: server_url.into(),
        scopes: vec![],
        client_name: "puffer-test".into(),
        token_dir,
    }
}

/// Drive the interactive_login by intercepting the `open_browser` callback,
/// programmatically following the auth URL (which auto-redirects to the
/// callback) so the localhost callback receiver fires immediately.
async fn drive_interactive_login(service: &OAuthService) -> Result<(), OAuthError> {
    let client = reqwest::ClientBuilder::new()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    service
        .interactive_login(None, move |url| {
            // Spawn a background task that follows the redirect to land on
            // the callback. interactive_login is awaiting the callback
            // before this returns, so do this on a fresh tokio task to
            // avoid a deadlock.
            let url = url.to_string();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .unwrap();
                rt.block_on(async move {
                    let resp = client.get(&url).send().await.unwrap();
                    if resp.status().is_redirection() {
                        if let Some(loc) = resp.headers().get(reqwest::header::LOCATION) {
                            let location = loc.to_str().unwrap().to_string();
                            // Follow the redirect (which is the callback URL).
                            let _ = reqwest::Client::new().get(&location).send().await;
                        }
                    }
                });
            });
            Ok(())
        })
        .await
}

#[tokio::test]
async fn auth_code_flow_completes_against_stub() {
    let stub = spawn_oauth_stub(OAuthStubConfig::default()).await.unwrap();
    let dir = TempDir::new().unwrap();
    let service = OAuthService::new(config_for(&stub.base_url, dir.path().into()));

    drive_interactive_login(&service).await.expect("login");

    let metrics = stub.metrics().await;
    assert_eq!(metrics.register_hits, 1, "DCR should have run once");
    assert_eq!(metrics.authorize_hits, 1, "authorize should have run once");
    assert_eq!(metrics.auth_code_grants, 1, "code exchange should have run");
    assert!(service.has_stored_tokens(), "tokens persisted to disk");

    // resolve() should now succeed silently using the persisted tokens.
    let resolved = service.resolve().await.expect("resolve after login");
    assert!(resolved.client.get_access_token().await.is_ok());

    stub.shutdown().await;
}

#[tokio::test]
async fn tokens_persist_across_runner_restarts() {
    let stub = spawn_oauth_stub(OAuthStubConfig::default()).await.unwrap();
    let dir = TempDir::new().unwrap();

    {
        let service = OAuthService::new(config_for(&stub.base_url, dir.path().into()));
        drive_interactive_login(&service).await.expect("login");
    }

    // Drop the first service, build a fresh one against the same dir.
    let restart_metrics_before = stub.metrics().await;
    let service2 = OAuthService::new(config_for(&stub.base_url, dir.path().into()));
    let resolved = service2.resolve().await.expect("resolve after restart");
    let _ = resolved.client.get_access_token().await.expect("token");

    let metrics_after = stub.metrics().await;
    assert_eq!(
        metrics_after.authorize_hits, restart_metrics_before.authorize_hits,
        "second start must not re-run /authorize"
    );
    assert_eq!(
        metrics_after.register_hits, restart_metrics_before.register_hits,
        "second start must not re-run /register"
    );

    stub.shutdown().await;
}

#[tokio::test]
async fn token_refresh_on_expiry_succeeds() {
    let stub = spawn_oauth_stub(OAuthStubConfig {
        access_token_ttl: Duration::from_secs(1),
        ..Default::default()
    })
    .await
    .unwrap();
    let dir = TempDir::new().unwrap();
    let service = OAuthService::new(config_for(&stub.base_url, dir.path().into()));

    drive_interactive_login(&service).await.expect("login");
    // Wait past the 1s expiry + the 60s skew window to ensure the
    // resolve-time refresh fires deterministically.
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    let pre = stub.metrics().await;
    let resolved = service.resolve().await.expect("resolve with refresh");
    let post = stub.metrics().await;
    assert!(
        post.refresh_grants > pre.refresh_grants,
        "refresh grant should have been issued (pre={}, post={})",
        pre.refresh_grants,
        post.refresh_grants
    );
    let token = resolved
        .client
        .get_access_token()
        .await
        .expect("post-refresh token");
    assert!(token.starts_with("stub-access-"));

    stub.shutdown().await;
}

#[tokio::test]
async fn expired_refresh_token_returns_oauth_required() {
    let stub = spawn_oauth_stub(OAuthStubConfig {
        access_token_ttl: Duration::from_secs(1),
        fail_refresh: true,
    })
    .await
    .unwrap();
    let dir = TempDir::new().unwrap();
    let service = OAuthService::new(config_for(&stub.base_url, dir.path().into()));

    drive_interactive_login(&service).await.expect("login");
    tokio::time::sleep(Duration::from_millis(1_500)).await;

    match service.resolve().await {
        Err(OAuthError::OAuthRequired { server_id, .. }) => {
            assert_eq!(server_id, "stub-server");
        }
        Err(other) => panic!("expected OAuthRequired, got {other:?}"),
        Ok(_) => panic!("expected OAuthRequired, got Ok(_)"),
    }

    stub.shutdown().await;
}

#[tokio::test]
async fn dcr_registration_returns_client_id() {
    let stub = spawn_oauth_stub(OAuthStubConfig::default()).await.unwrap();
    let dir = TempDir::new().unwrap();
    let service = OAuthService::new(config_for(&stub.base_url, dir.path().into()));
    drive_interactive_login(&service).await.expect("login");
    let persisted = puffer_mcp_oauth::FileCredentialStore::new(
        dir.path().to_path_buf(),
        "stub-server",
        &stub.base_url,
    );
    persisted.prime_from_disk();
    use rmcp::transport::auth::CredentialStore;
    let stored = persisted.load().await.unwrap().expect("stored");
    assert!(stored.client_id.starts_with("stub-client-"));
    stub.shutdown().await;
}

#[tokio::test]
async fn pkce_code_verifier_challenge_round_trip() {
    // The auth_code_flow_completes_against_stub test already covers the
    // happy path. This one specifically tampers: the stub rejects the
    // grant if PKCE doesn't match, so a missing/wrong verifier surfaces
    // as a TokenExchange error.
    let stub = spawn_oauth_stub(OAuthStubConfig::default()).await.unwrap();
    // Manually drive: hit /authorize, get the code, then send a /token
    // request with a wrong verifier and assert 400.
    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    // Need a registered client_id first.
    let reg: serde_json::Value = client
        .post(format!("{}/register", stub.base_url))
        .json(&serde_json::json!({
            "client_name": "test",
            "redirect_uris": ["http://127.0.0.1:1/callback"],
            "grant_types": ["authorization_code"],
            "response_types": ["code"],
            "token_endpoint_auth_method": "none",
        }))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    let client_id = reg["client_id"].as_str().unwrap().to_string();
    // Use a fixed verifier+challenge from RFC 7636 sample.
    let verifier = "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk";
    let challenge = "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM";
    let auth_url = format!(
        "{}/authorize?response_type=code&client_id={}&redirect_uri=http%3A%2F%2F127.0.0.1%3A1%2Fcallback&state=xyz&code_challenge={}&code_challenge_method=S256",
        stub.base_url, client_id, challenge
    );
    let resp = client.get(&auth_url).send().await.unwrap();
    assert!(resp.status().is_redirection());
    let location = resp.headers().get("location").unwrap().to_str().unwrap();
    let code = url::Url::parse(location)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();

    // Wrong verifier -> 400
    let bad = client
        .post(format!("{}/token", stub.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code),
            ("code_verifier", "wrong-verifier"),
            ("redirect_uri", "http://127.0.0.1:1/callback"),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);

    // Right verifier -> 200 (use a fresh code since the previous one was consumed).
    let resp2 = client.get(&auth_url).send().await.unwrap();
    let location2 = resp2.headers().get("location").unwrap().to_str().unwrap();
    let code2 = url::Url::parse(location2)
        .unwrap()
        .query_pairs()
        .find(|(k, _)| k == "code")
        .unwrap()
        .1
        .to_string();
    let good = client
        .post(format!("{}/token", stub.base_url))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", &code2),
            ("code_verifier", verifier),
            ("redirect_uri", "http://127.0.0.1:1/callback"),
            ("client_id", &client_id),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(good.status(), 200);

    stub.shutdown().await;
}

#[tokio::test]
async fn oauth_discovery_finds_authorization_server() {
    let stub = spawn_oauth_stub(OAuthStubConfig::default()).await.unwrap();
    let resp: serde_json::Value = reqwest::get(format!(
        "{}/.well-known/oauth-authorization-server",
        stub.base_url
    ))
    .await
    .unwrap()
    .json()
    .await
    .unwrap();
    assert!(resp["authorization_endpoint"]
        .as_str()
        .unwrap()
        .contains("/authorize"));
    assert!(resp["token_endpoint"].as_str().unwrap().contains("/token"));
    assert!(resp["registration_endpoint"]
        .as_str()
        .unwrap()
        .contains("/register"));
    stub.shutdown().await;
}
