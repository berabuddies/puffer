use crate::auth::{OAuthCredential, StoredCredential};
use anyhow::{Context, Result};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Distinguishes external credential sources that Puffer can import.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalImportSource {
    Claude,
    Codex,
}

/// Groups import candidates by the provider family they can satisfy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExternalImportFamily {
    Anthropic,
    OpenAi,
}

/// Describes one importable credential source discovered on disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ExternalImportCandidate {
    pub source: ExternalImportSource,
    pub family: ExternalImportFamily,
    pub description: String,
    pub source_path: PathBuf,
    pub credential: StoredCredential,
    pub openai_base_url: Option<String>,
    pub openai_headers: BTreeMap<String, String>,
    pub openai_query_params: BTreeMap<String, String>,
}

/// Detects importable external credentials for the requested provider family.
pub fn detect_import_candidates(
    family: ExternalImportFamily,
) -> Result<Vec<ExternalImportCandidate>> {
    let Some(home) = import_home_dir() else {
        return Ok(Vec::new());
    };
    let mut candidates = Vec::new();
    if family == ExternalImportFamily::Anthropic {
        candidates.extend(read_claude_candidates(&home)?);
    }
    if family == ExternalImportFamily::OpenAi {
        candidates.extend(read_codex_candidates(&home)?);
    }
    Ok(candidates)
}

fn import_home_dir() -> Option<PathBuf> {
    std::env::var_os("PUFFER_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(PathBuf::from))
        .or_else(dirs::home_dir)
}

fn read_claude_candidates(home: &Path) -> Result<Vec<ExternalImportCandidate>> {
    let mut candidates = Vec::new();
    let credentials_path = home.join(".claude").join(".credentials.json");
    if credentials_path.exists() {
        let raw = fs::read_to_string(&credentials_path)
            .with_context(|| format!("failed to read {}", credentials_path.display()))?;
        let storage: ClaudeCredentialStore = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", credentials_path.display()))?;
        if let Some(oauth) = storage.claude_ai_oauth {
            let email = oauth.email();
            let account_id = oauth.account_id();
            let organization_id = oauth.organization_id();
            let mut description = String::from("Import Claude OAuth");
            if let Some(email) = email.as_deref() {
                description.push_str(&format!(" ({email})"));
            }
            candidates.push(ExternalImportCandidate {
                source: ExternalImportSource::Claude,
                family: ExternalImportFamily::Anthropic,
                description,
                source_path: credentials_path.clone(),
                credential: StoredCredential::OAuth(OAuthCredential {
                    access_token: oauth.access_token,
                    refresh_token: oauth.refresh_token.unwrap_or_default(),
                    expires_at_ms: oauth.expires_at.unwrap_or_default(),
                    account_id,
                    organization_id,
                    email,
                    plan_type: oauth.subscription_type,
                    rate_limit_tier: oauth.rate_limit_tier,
                    scopes: oauth.scopes,
                    organization_name: None,
                    organization_role: None,
                    workspace_role: None,
                }),
                openai_base_url: None,
                openai_headers: BTreeMap::new(),
                openai_query_params: BTreeMap::new(),
            });
        }
    }

    let config_path = home.join(".claude.json");
    if config_path.exists() {
        let raw = fs::read_to_string(&config_path)
            .with_context(|| format!("failed to read {}", config_path.display()))?;
        let config: ClaudeGlobalConfig = serde_json::from_str(&raw)
            .with_context(|| format!("failed to parse {}", config_path.display()))?;
        if let Some(api_key) = config
            .primary_api_key
            .filter(|value| !value.trim().is_empty())
        {
            candidates.push(ExternalImportCandidate {
                source: ExternalImportSource::Claude,
                family: ExternalImportFamily::Anthropic,
                description: "Import Claude API key".to_string(),
                source_path: config_path,
                credential: StoredCredential::ApiKey { key: api_key },
                openai_base_url: None,
                openai_headers: BTreeMap::new(),
                openai_query_params: BTreeMap::new(),
            });
        }
    }

    Ok(candidates)
}

fn read_codex_candidates(home: &Path) -> Result<Vec<ExternalImportCandidate>> {
    let auth_path = home.join(".codex").join("auth.json");
    let config_path = home.join(".codex").join("config.toml");
    let imported_provider = read_codex_openai_import(&config_path)?;
    let mut candidates = Vec::new();
    if let Some(bearer_token) = imported_provider.experimental_bearer_token.as_deref() {
        candidates.push(ExternalImportCandidate {
            source: ExternalImportSource::Codex,
            family: ExternalImportFamily::OpenAi,
            description: "Import Codex experimental bearer token".to_string(),
            source_path: config_path,
            credential: StoredCredential::ApiKey {
                key: bearer_token.to_string(),
            },
            openai_base_url: imported_provider.base_url,
            openai_headers: imported_provider.headers,
            openai_query_params: imported_provider.query_params,
        });
        return Ok(candidates);
    }

    if !auth_path.exists() {
        return Ok(candidates);
    }
    let raw = fs::read_to_string(&auth_path)
        .with_context(|| format!("failed to read {}", auth_path.display()))?;
    let auth: CodexAuthFile = serde_json::from_str(&raw)
        .with_context(|| format!("failed to parse {}", auth_path.display()))?;

    if let Some(api_key) = auth.openai_api_key.filter(|value| !value.trim().is_empty()) {
        candidates.push(ExternalImportCandidate {
            source: ExternalImportSource::Codex,
            family: ExternalImportFamily::OpenAi,
            description: "Import Codex API key".to_string(),
            source_path: auth_path.clone(),
            credential: StoredCredential::ApiKey { key: api_key },
            openai_base_url: imported_provider.base_url.clone(),
            openai_headers: imported_provider.headers.clone(),
            openai_query_params: imported_provider.query_params.clone(),
        });
    }
    if let Some(tokens) = auth.tokens {
        let claims = parse_codex_id_token(tokens.id_token.as_deref());
        let plan = claims.as_ref().and_then(|claim| claim.plan_type.clone());
        let email = claims.as_ref().and_then(|claim| claim.email.clone());
        let account_id = tokens
            .account_id
            .or_else(|| claims.and_then(|claim| claim.account_id));
        let mut description = String::from("Import Codex OAuth");
        if let Some(email) = email.as_deref() {
            description.push_str(&format!(" ({email})"));
        }
        if let Some(base_url) = imported_provider.base_url.as_deref() {
            description.push_str(&format!(" via {base_url}"));
        }
        candidates.push(ExternalImportCandidate {
            source: ExternalImportSource::Codex,
            family: ExternalImportFamily::OpenAi,
            description,
            source_path: auth_path,
            credential: StoredCredential::OAuth(OAuthCredential {
                access_token: tokens.access_token,
                refresh_token: tokens.refresh_token,
                expires_at_ms: 0,
                account_id,
                organization_id: None,
                email,
                plan_type: plan,
                rate_limit_tier: None,
                scopes: Vec::new(),
                organization_name: None,
                organization_role: None,
                workspace_role: None,
            }),
            openai_base_url: imported_provider.base_url,
            openai_headers: imported_provider.headers,
            openai_query_params: imported_provider.query_params,
        });
    }
    Ok(candidates)
}

fn read_codex_openai_import(config_path: &Path) -> Result<CodexOpenAiImport> {
    if !config_path.exists() {
        return Ok(CodexOpenAiImport::default());
    }
    let raw = fs::read_to_string(config_path)
        .with_context(|| format!("failed to read {}", config_path.display()))?;
    let config: CodexConfigToml = toml::from_str(&raw)
        .with_context(|| format!("failed to parse {}", config_path.display()))?;
    Ok(resolve_codex_openai_import(&config))
}

fn resolve_codex_openai_import(config: &CodexConfigToml) -> CodexOpenAiImport {
    let base_url = config
        .openai_base_url
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string)
        .or_else(|| {
            let selected_provider = config.model_provider.as_deref()?;
            let provider = config.model_providers.as_ref()?.get(selected_provider)?;
            provider
                .base_url
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToString::to_string)
        });
    let provider = config
        .model_provider
        .as_deref()
        .and_then(|selected_provider| config.model_providers.as_ref()?.get(selected_provider));
    let headers = provider
        .map(resolve_codex_provider_headers)
        .unwrap_or_default();
    let query_params = provider
        .and_then(|provider| provider.query_params.clone())
        .unwrap_or_default();
    let experimental_bearer_token = provider
        .and_then(|provider| provider.experimental_bearer_token.as_deref())
        .or(config.experimental_bearer_token.as_deref())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToString::to_string);
    CodexOpenAiImport {
        base_url,
        headers,
        query_params,
        experimental_bearer_token,
    }
}

fn resolve_codex_provider_headers(provider: &CodexModelProviderToml) -> BTreeMap<String, String> {
    let mut headers = provider.http_headers.clone().unwrap_or_default();
    for (header, env_var) in provider.env_http_headers.clone().unwrap_or_default() {
        if headers.contains_key(&header) {
            continue;
        }
        if let Ok(value) = std::env::var(&env_var) {
            let trimmed = value.trim();
            if !trimmed.is_empty() {
                headers.insert(header, trimmed.to_string());
            }
        }
    }
    headers
}

fn parse_codex_id_token(raw_jwt: Option<&str>) -> Option<CodexIdClaims> {
    let raw_jwt = raw_jwt?;
    let payload = raw_jwt.split('.').nth(1)?;
    let decoded = URL_SAFE_NO_PAD.decode(payload.as_bytes()).ok()?;
    let json = serde_json::from_slice::<Value>(&decoded).ok()?;
    let email = json
        .get("email")
        .and_then(Value::as_str)
        .map(ToString::to_string)
        .or_else(|| {
            json.pointer("/https://api.openai.com/profile/email")
                .and_then(Value::as_str)
                .map(ToString::to_string)
        });
    let plan_type = json
        .get("https://api.openai.com/auth")
        .and_then(|claim| claim.get("chatgpt_plan_type"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    let account_id = json
        .get("https://api.openai.com/auth")
        .and_then(|claim| claim.get("chatgpt_account_id"))
        .and_then(Value::as_str)
        .map(ToString::to_string);
    Some(CodexIdClaims {
        email,
        account_id,
        plan_type,
    })
}

#[derive(Debug, Deserialize)]
struct ClaudeCredentialStore {
    #[serde(rename = "claudeAiOauth")]
    claude_ai_oauth: Option<ClaudeOauthTokens>,
}

#[derive(Debug, Deserialize)]
struct ClaudeOauthTokens {
    #[serde(rename = "accessToken")]
    access_token: String,
    #[serde(rename = "refreshToken")]
    refresh_token: Option<String>,
    #[serde(rename = "expiresAt")]
    expires_at: Option<u64>,
    #[serde(default)]
    scopes: Vec<String>,
    #[serde(rename = "subscriptionType")]
    subscription_type: Option<String>,
    #[serde(rename = "rateLimitTier")]
    rate_limit_tier: Option<String>,
    #[serde(default)]
    email: Option<String>,
    #[serde(rename = "emailAddress", default)]
    email_address: Option<String>,
    #[serde(rename = "accountUuid", default)]
    account_uuid: Option<String>,
    #[serde(rename = "organizationUuid", default)]
    organization_uuid: Option<String>,
}

impl ClaudeOauthTokens {
    fn email(&self) -> Option<String> {
        self.email.clone().or(self.email_address.clone())
    }

    fn account_id(&self) -> Option<String> {
        self.account_uuid.clone()
    }

    fn organization_id(&self) -> Option<String> {
        self.organization_uuid.clone()
    }
}

#[derive(Debug, Deserialize)]
struct ClaudeGlobalConfig {
    #[serde(rename = "primaryApiKey")]
    primary_api_key: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexAuthFile {
    #[serde(rename = "OPENAI_API_KEY")]
    openai_api_key: Option<String>,
    #[serde(default)]
    tokens: Option<CodexTokens>,
}

#[derive(Debug, Deserialize, Default)]
struct CodexConfigToml {
    #[serde(default)]
    openai_base_url: Option<String>,
    #[serde(default)]
    experimental_bearer_token: Option<String>,
    #[serde(default)]
    model_provider: Option<String>,
    #[serde(default)]
    model_providers: Option<BTreeMap<String, CodexModelProviderToml>>,
}

#[derive(Debug, Deserialize, Default)]
struct CodexModelProviderToml {
    #[serde(default)]
    base_url: Option<String>,
    #[serde(default)]
    experimental_bearer_token: Option<String>,
    #[serde(default)]
    http_headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    env_http_headers: Option<BTreeMap<String, String>>,
    #[serde(default)]
    query_params: Option<BTreeMap<String, String>>,
}

#[derive(Debug, Default)]
struct CodexOpenAiImport {
    base_url: Option<String>,
    headers: BTreeMap<String, String>,
    query_params: BTreeMap<String, String>,
    experimental_bearer_token: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CodexTokens {
    access_token: String,
    refresh_token: String,
    #[serde(default)]
    account_id: Option<String>,
    #[serde(default)]
    id_token: Option<String>,
}

#[derive(Debug)]
struct CodexIdClaims {
    email: Option<String>,
    account_id: Option<String>,
    plan_type: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Mutex, OnceLock};
    use std::time::{SystemTime, UNIX_EPOCH};

    fn puffer_home_lock() -> &'static Mutex<()> {
        static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
        LOCK.get_or_init(|| Mutex::new(()))
    }

    #[test]
    fn parse_codex_id_token_extracts_email_account_and_plan() {
        let payload = serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-123",
                "chatgpt_plan_type": "pro"
            }
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{encoded}.sig");
        let claims = parse_codex_id_token(Some(&token)).expect("claims");
        assert_eq!(claims.email.as_deref(), Some("dev@example.com"));
        assert_eq!(claims.account_id.as_deref(), Some("acct-123"));
        assert_eq!(claims.plan_type.as_deref(), Some("pro"));
    }

    #[test]
    fn detect_import_candidates_reads_codex_auth_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _lock = puffer_home_lock().lock().expect("lock");
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("dir");
        let payload = serde_json::json!({
            "email": "dev@example.com",
            "https://api.openai.com/auth": {
                "chatgpt_account_id": "acct-123",
                "chatgpt_plan_type": "pro"
            }
        });
        let encoded = URL_SAFE_NO_PAD.encode(payload.to_string());
        let token = format!("header.{encoded}.sig");
        fs::write(
            codex_dir.join("auth.json"),
            serde_json::json!({
                "OPENAI_API_KEY": "sk-openai",
                "tokens": {
                    "access_token": "acc",
                    "refresh_token": "ref",
                    "id_token": token
                }
            })
            .to_string(),
        )
        .expect("write");
        fs::write(
            codex_dir.join("config.toml"),
            r#"
openai_base_url = "https://proxy.example/v1"
"#,
        )
        .expect("write");
        let old_home = std::env::var_os("PUFFER_HOME");
        std::env::set_var("PUFFER_HOME", temp.path());
        let candidates =
            detect_import_candidates(ExternalImportFamily::OpenAi).expect("candidates");
        if let Some(value) = old_home {
            std::env::set_var("PUFFER_HOME", value);
        } else {
            std::env::remove_var("PUFFER_HOME");
        }
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|candidate| matches!(
            &candidate.credential,
            StoredCredential::ApiKey { key } if key == "sk-openai"
        )));
        assert!(candidates.iter().any(|candidate| matches!(
            &candidate.credential,
            StoredCredential::OAuth(credential)
                if credential.email.as_deref() == Some("dev@example.com")
                    && credential.plan_type.as_deref() == Some("pro")
        )));
        assert!(candidates.iter().all(|candidate| {
            candidate.openai_base_url.as_deref() == Some("https://proxy.example/v1")
        }));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.openai_headers.is_empty()));
        assert!(candidates
            .iter()
            .all(|candidate| candidate.openai_query_params.is_empty()));
    }

    #[test]
    fn detect_import_candidates_reads_codex_experimental_bearer_without_auth_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _lock = puffer_home_lock().lock().expect("lock");
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("dir");
        fs::write(
            codex_dir.join("config.toml"),
            r#"
model_provider = "OpenAI"

[model_providers.OpenAI]
base_url = "https://proxy.example/v1"
experimental_bearer_token = "  sk-experimental  "
"#,
        )
        .expect("write");
        let old_home = std::env::var_os("PUFFER_HOME");
        std::env::set_var("PUFFER_HOME", temp.path());
        let candidates =
            detect_import_candidates(ExternalImportFamily::OpenAi).expect("candidates");
        if let Some(value) = old_home {
            std::env::set_var("PUFFER_HOME", value);
        } else {
            std::env::remove_var("PUFFER_HOME");
        }

        assert_eq!(candidates.len(), 1);
        let candidate = candidates.first().expect("candidate");
        assert_eq!(candidate.source_path, codex_dir.join("config.toml"));
        assert_eq!(
            candidate.openai_base_url.as_deref(),
            Some("https://proxy.example/v1")
        );
        assert!(matches!(
            &candidate.credential,
            StoredCredential::ApiKey { key } if key == "sk-experimental"
        ));
    }

    #[test]
    fn detect_import_candidates_prefers_codex_experimental_bearer_over_auth_file() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _lock = puffer_home_lock().lock().expect("lock");
        let codex_dir = temp.path().join(".codex");
        fs::create_dir_all(&codex_dir).expect("dir");
        fs::write(
            codex_dir.join("auth.json"),
            serde_json::json!({
                "OPENAI_API_KEY": "sk-auth-json",
                "tokens": {
                    "access_token": "acc",
                    "refresh_token": "ref"
                }
            })
            .to_string(),
        )
        .expect("write");
        fs::write(
            codex_dir.join("config.toml"),
            r#"
openai_base_url = "https://proxy.example/v1"
experimental_bearer_token = "sk-config"
"#,
        )
        .expect("write");
        let old_home = std::env::var_os("PUFFER_HOME");
        std::env::set_var("PUFFER_HOME", temp.path());
        let candidates =
            detect_import_candidates(ExternalImportFamily::OpenAi).expect("candidates");
        if let Some(value) = old_home {
            std::env::set_var("PUFFER_HOME", value);
        } else {
            std::env::remove_var("PUFFER_HOME");
        }

        assert_eq!(candidates.len(), 1);
        assert!(matches!(
            &candidates[0].credential,
            StoredCredential::ApiKey { key } if key == "sk-config"
        ));
    }

    #[test]
    fn resolve_codex_openai_import_falls_back_to_selected_provider_base_url() {
        let config: CodexConfigToml = toml::from_str(
            r#"
model_provider = "corp"

[model_providers.corp]
base_url = "https://corp-proxy.example/v1"
"#,
        )
        .expect("config");

        let resolved = resolve_codex_openai_import(&config);
        assert_eq!(
            resolved.base_url.as_deref(),
            Some("https://corp-proxy.example/v1")
        );
        assert!(resolved.headers.is_empty());
        assert!(resolved.query_params.is_empty());
    }

    #[test]
    fn resolve_codex_openai_import_reads_http_and_env_headers() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _lock = puffer_home_lock().lock().expect("lock");
        let old_header = std::env::var_os("PUFFER_CODEX_IMPORT_TEST_HEADER");
        std::env::set_var("PUFFER_CODEX_IMPORT_TEST_HEADER", "from-env");
        let config: CodexConfigToml = toml::from_str(
            r#"
model_provider = "corp"

[model_providers.corp]
base_url = "https://corp-proxy.example/v1"

[model_providers.corp.http_headers]
x-static = "static-value"

[model_providers.corp.env_http_headers]
x-env = "PUFFER_CODEX_IMPORT_TEST_HEADER"

[model_providers.corp.query_params]
api-version = "2025-01-01"
"#,
        )
        .expect("config");

        let resolved = resolve_codex_openai_import(&config);
        assert_eq!(
            resolved.headers.get("x-static").map(String::as_str),
            Some("static-value")
        );
        assert_eq!(
            resolved.headers.get("x-env").map(String::as_str),
            Some("from-env")
        );
        assert_eq!(
            resolved.query_params.get("api-version").map(String::as_str),
            Some("2025-01-01")
        );
        drop(temp);
        if let Some(value) = old_header {
            std::env::set_var("PUFFER_CODEX_IMPORT_TEST_HEADER", value);
        } else {
            std::env::remove_var("PUFFER_CODEX_IMPORT_TEST_HEADER");
        }
    }

    #[test]
    fn detect_import_candidates_reads_claude_files() {
        let temp = tempfile::tempdir().expect("tempdir");
        let _lock = puffer_home_lock().lock().expect("lock");
        let claude_dir = temp.path().join(".claude");
        fs::create_dir_all(&claude_dir).expect("dir");
        fs::write(
            claude_dir.join(".credentials.json"),
            serde_json::json!({
                "claudeAiOauth": {
                    "accessToken": "acc",
                    "refreshToken": "ref",
                    "expiresAt": SystemTime::now().duration_since(UNIX_EPOCH).expect("time").as_millis() as u64,
                    "scopes": ["user:profile", "user:inference"],
                    "subscriptionType": "max",
                    "rateLimitTier": "tier-1",
                    "emailAddress": "dev@example.com",
                    "accountUuid": "acct-123",
                    "organizationUuid": "org-123"
                }
            })
            .to_string(),
        )
        .expect("write");
        fs::write(
            temp.path().join(".claude.json"),
            serde_json::json!({
                "primaryApiKey": "sk-ant"
            })
            .to_string(),
        )
        .expect("write");
        let old_home = std::env::var_os("PUFFER_HOME");
        std::env::set_var("PUFFER_HOME", temp.path());
        let candidates =
            detect_import_candidates(ExternalImportFamily::Anthropic).expect("candidates");
        if let Some(value) = old_home {
            std::env::set_var("PUFFER_HOME", value);
        } else {
            std::env::remove_var("PUFFER_HOME");
        }
        assert_eq!(candidates.len(), 2);
        assert!(candidates.iter().any(|candidate| matches!(
            &candidate.credential,
            StoredCredential::ApiKey { key } if key == "sk-ant"
        )));
        assert!(candidates.iter().any(|candidate| matches!(
            &candidate.credential,
            StoredCredential::OAuth(credential)
                if credential.email.as_deref() == Some("dev@example.com")
                    && credential.organization_id.as_deref() == Some("org-123")
                    && credential.plan_type.as_deref() == Some("max")
        )));
    }
}
