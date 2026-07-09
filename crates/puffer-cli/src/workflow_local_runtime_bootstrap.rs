use super::{
    trimmed, RuntimeContext, AGENTENV_LOCAL_RUNTIME_IMAGE, LOCAL_WORKFLOW_RUNTIME_API_PORT,
    POSTGRES_IMAGE, POSTGRES_URL, REDIS_IMAGE, REDIS_URL,
};
use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs;
use std::path::Path;
use uuid::Uuid;

/// Writes the generated Compose, environment, and seed files for the runtime.
pub(super) fn write_runtime_files(runtime: &RuntimeContext) -> Result<()> {
    fs::create_dir_all(&runtime.stack_dir).with_context(|| {
        format!(
            "create local workflow runtime stack dir {}",
            runtime.stack_dir.display()
        )
    })?;
    fs::write(&runtime.compose_file, compose_file_text(runtime)).with_context(|| {
        format!(
            "write local workflow runtime compose file {}",
            runtime.compose_file.display()
        )
    })?;
    fs::write(&runtime.env_file, env_file_text(runtime)).with_context(|| {
        format!(
            "write local workflow runtime env file {}",
            runtime.env_file.display()
        )
    })?;
    fs::write(&runtime.seed_file, seed_sql_text(runtime)).with_context(|| {
        format!(
            "write local workflow runtime seed SQL {}",
            runtime.seed_file.display()
        )
    })?;
    Ok(())
}

fn compose_file_text(runtime: &RuntimeContext) -> String {
    format!(
        r#"name: {project}
services:
  postgres:
    image: {postgres_image}
    environment:
      POSTGRES_USER: tintin
      POSTGRES_PASSWORD: tintin
      POSTGRES_DB: tintin_cloud
    volumes:
      - ./data/postgres:/var/lib/postgresql/data
    healthcheck:
      test: ["CMD-SHELL", "pg_isready -U tintin -d tintin_cloud"]
      interval: 2s
      timeout: 5s
      retries: 30

  redis:
    image: {redis_image}
    command: ["redis-server", "--appendonly", "yes"]
    volumes:
      - ./data/redis:/data
    healthcheck:
      test: ["CMD", "redis-cli", "ping"]
      interval: 2s
      timeout: 5s
      retries: 30

  migrate:
    image: {agentenv_image}
    env_file:
      - .env
    command: ["node", "dist/database/migrate.js"]
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy

  seed:
    image: {postgres_image}
    env_file:
      - .env
    command: ["sh", "-c", "psql \"$$DATABASE_URL\" -f /bootstrap/seed.sql"]
    volumes:
      - ./bootstrap/seed.sql:/bootstrap/seed.sql:ro
    depends_on:
      postgres:
        condition: service_healthy

  api:
    image: {agentenv_image}
    env_file:
      - .env
    depends_on:
      postgres:
        condition: service_healthy
      redis:
        condition: service_healthy
    ports:
      - "127.0.0.1:{host_port}:{api_port}"
"#,
        agentenv_image = AGENTENV_LOCAL_RUNTIME_IMAGE,
        api_port = LOCAL_WORKFLOW_RUNTIME_API_PORT,
        host_port = runtime.host_port,
        postgres_image = POSTGRES_IMAGE,
        project = runtime.stack_name,
        redis_image = REDIS_IMAGE,
    )
}

fn env_file_text(runtime: &RuntimeContext) -> String {
    format!(
        "NODE_ENV=development\nGRPC_USE_TLS=false\nSCHEDULER_PROTO_PATH=/app/protos/scheduler/scheduler.proto\nHYPERVISOR_PROTO_PATH=/app/protos/hypervisor/hypervisor.proto\nDATABASE_URL={POSTGRES_URL}\nREDIS_URL={REDIS_URL}\nAPI_KEY_PEPPER={}\nGATEWAY_ENCRYPTION_KEY={}\nJWT_SECRET={}\nJWT_REFRESH_SECRET={}\nLOCAL_USER_ID={}\nLOCAL_WORKSPACE_ID={}\n",
        runtime.api_key_pepper,
        runtime.gateway_encryption_key,
        runtime.jwt_secret,
        runtime.jwt_refresh_secret,
        runtime.user_id,
        runtime.workspace_id
    )
}

fn seed_sql_text(runtime: &RuntimeContext) -> String {
    let key_hash = api_key_hash(&runtime.api_key_pepper, &runtime.api_key);
    let api_key_id = stable_uuid("api-key", &runtime.workspace_id);
    let email = format!("puffer-local-{}@puffer.local", runtime.user_id);
    let username = format!("puffer_local_{}", runtime.user_id.replace('-', ""));
    format!(
        r#"\set ON_ERROR_STOP on
BEGIN;

INSERT INTO users (id, email, username, roles, permissions, "emailVerified", "isActive")
VALUES ({user_id}, {email}, {username}, '[]'::json, '[]'::json, true, true)
ON CONFLICT (id) DO UPDATE SET
  email = EXCLUDED.email,
  username = EXCLUDED.username,
  "emailVerified" = true,
  "isActive" = true,
  "updatedAt" = now();

INSERT INTO workspaces (id, "ownerId", name, "deletedAt")
VALUES ({workspace_id}, {user_id}, 'Puffer Local', NULL)
ON CONFLICT (id) DO UPDATE SET
  "ownerId" = EXCLUDED."ownerId",
  name = EXCLUDED.name,
  "deletedAt" = NULL,
  "updatedAt" = now();

INSERT INTO user_workspaces ("userId", "workspaceId", role)
VALUES ({user_id}, {workspace_id}, 'owner'::user_workspaces_role_enum)
ON CONFLICT ("userId", "workspaceId") DO UPDATE SET
  role = EXCLUDED.role;

INSERT INTO api_keys (id, "userId", type, "workspaceId", name, "keyHash", "revokedAt")
VALUES ({api_key_id}, {user_id}, 'user', NULL, 'Puffer Local', {key_hash}, NULL)
ON CONFLICT (id) DO UPDATE SET
  "userId" = EXCLUDED."userId",
  type = EXCLUDED.type,
  "workspaceId" = EXCLUDED."workspaceId",
  name = EXCLUDED.name,
  "keyHash" = EXCLUDED."keyHash",
  "revokedAt" = NULL,
  "updatedAt" = now();

COMMIT;
"#,
        api_key_id = sql_string(&api_key_id.to_string()),
        email = sql_string(&email),
        key_hash = sql_string(&key_hash),
        user_id = sql_string(&runtime.user_id),
        username = sql_string(&username),
        workspace_id = sql_string(&runtime.workspace_id),
    )
}

/// Reads a simple key-value `.env` file into a map.
pub(super) fn read_env_file(path: &Path) -> Result<BTreeMap<String, String>> {
    if !path.exists() {
        return Ok(BTreeMap::new());
    }
    let raw = fs::read_to_string(path)
        .with_context(|| format!("read local workflow runtime env file {}", path.display()))?;
    Ok(raw
        .lines()
        .filter_map(|line| line.split_once('='))
        .map(|(key, value)| (key.trim().to_string(), value.trim().to_string()))
        .filter(|(key, _)| !key.is_empty() && !key.starts_with('#'))
        .collect())
}

/// Returns a trimmed non-empty environment value from the parsed `.env` map.
pub(super) fn non_empty_env(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    env.get(key)
        .map(String::as_str)
        .and_then(trimmed)
        .map(ToString::to_string)
}

/// Returns a parsed UUID environment value from the parsed `.env` map.
pub(super) fn valid_uuid_env(env: &BTreeMap<String, String>, key: &str) -> Option<String> {
    let value = non_empty_env(env, key)?;
    Uuid::parse_str(&value).ok()?;
    Some(value)
}

fn api_key_hash(pepper: &str, api_key: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(pepper.as_bytes());
    hasher.update(api_key.as_bytes());
    format!("{:x}", hasher.finalize())
}

fn stable_uuid(namespace: &str, value: &str) -> Uuid {
    let mut hasher = Sha256::new();
    hasher.update(namespace.as_bytes());
    hasher.update(b":");
    hasher.update(value.as_bytes());
    let digest = hasher.finalize();
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

fn sql_string(value: &str) -> String {
    format!("'{}'", value.replace('\'', "''"))
}
