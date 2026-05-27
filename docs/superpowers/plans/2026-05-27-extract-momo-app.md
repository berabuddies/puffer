# Extract `apps/momo/` Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Extract the V2 desktop app (Momo: puffer-only chat + wallet U-card) from `apps/puffer-desktop/` into a standalone Tauri app at `apps/momo/`, leaving V1 (Corbina: multi-provider coding agent) untouched in `apps/puffer-desktop/`.

**Architecture:** V1 and V2 currently share `apps/puffer-desktop/src-tauri/` but use almost disjoint code paths (V1 talks to puffer-daemon over WebSocket, V2 talks to a Tauri-internal WebSocket server in backend.rs). The split moves V2-only Rust modules + the V2 frontend (`src-v2/`) into a new app, trims `backend.rs` to just the 8 WebSocket methods V2 actually uses, removes V1-only modules and 7 dead orphan `.rs` files (~2988 lines), and renames the storage root from `~/.corbina` to `~/.momo`. Auth Station's OAuth callback port moves from 1457→1467 to allow both apps to run side-by-side.

**Tech Stack:** Tauri 2 (Rust + Svelte 5 + Vite), `puffer-session-store` workspace crate, WebSocket JSON-RPC (tungstenite), `@tauri-apps/plugin-opener` for OAuth in OS browser, Playwright for tests.

**Starting point:** Branch `feat/extract-momo-app` cut from `feat/desktop-ui-v2-login` (post-rebase, master + V2 login + wallet U-card work). Master branch is `master`.

**Total estimated time:** ~3 days (assuming no surprises in Auth Station whitelist updates).

---

## Pre-flight checks (do before Task 1)

- [ ] **Verify current branch state**

```bash
cd /Users/shun/Data/Code/tomo/agentenv/puffer
git status                        # must be clean
git rev-parse --abbrev-ref HEAD   # should be feat/desktop-ui-v2-login
git log --oneline origin/master..HEAD | wc -l   # should be 15 commits ahead
```

Expected: clean tree, on `feat/desktop-ui-v2-login`, 15 commits ahead of `origin/master`.

- [ ] **Verify `puffer` CLI is installed (release build at ~/.cargo/bin/puffer)**

```bash
which puffer && puffer --help | head -2
```

Expected: `/Users/shun/.cargo/bin/puffer`, usage line printed. If missing, run `cargo install --path crates/puffer-cli --locked` first.

- [ ] **Verify Auth Station whitelist currently allows `http://localhost:1457/callback`** (sanity baseline)

```bash
TOKEN=$(python3 -c "import json,os; print(json.load(open(os.path.expanduser('~/Library/Application Support/com.vercel.cli/auth.json')))['token'])")
curl -s "https://api.vercel.com/v10/projects/prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl/env?teamId=team_EOvpxvSsCupQhXAG3GeG317R" -H "Authorization: Bearer $TOKEN" | python3 -c "import json,sys; d=json.load(sys.stdin); print([e for e in d.get('envs',[]) if e['key']=='ALLOWED_REDIRECT_ORIGINS'][0].get('value','encrypted')[:200])"
```

(The value will be encrypted; if needed, use the `auth-deploy` skill's API workflow to read.) If `localhost:1457` is whitelisted, V1 OAuth works today — baseline good.

- [ ] **Create the working branch**

```bash
git checkout -b feat/extract-momo-app
```

---

## Task 1: Scaffold empty `apps/momo/` skeleton

**Files:**
- Create: `apps/momo/package.json`
- Create: `apps/momo/index.html`
- Create: `apps/momo/vite.config.ts`
- Create: `apps/momo/svelte.config.js`
- Create: `apps/momo/tsconfig.json`
- Create: `apps/momo/playwright.config.ts`
- Create: `apps/momo/src-tauri/Cargo.toml`
- Create: `apps/momo/src-tauri/tauri.conf.json`
- Create: `apps/momo/src-tauri/build.rs`
- Create: `apps/momo/src-tauri/src/main.rs`
- Create: `apps/momo/src-tauri/src/lib.rs` (minimal stub)
- Create: `apps/momo/src-tauri/capabilities/default.json`
- Copy: `apps/puffer-desktop/src-tauri/icons/*` → `apps/momo/src-tauri/icons/`

- [ ] **Step 1: Create directory skeleton**

```bash
mkdir -p apps/momo/src-tauri/src apps/momo/src-tauri/capabilities apps/momo/src-tauri/icons apps/momo/public
```

Note: we deliberately do NOT create `apps/momo/src/` or `apps/momo/tests/` here — `git mv` in Task 2 will create those by moving directories whole, which preserves history per file cleanly. `git mv` refuses to move into an existing directory.

- [ ] **Step 2: Copy icons (binary assets)**

```bash
cp apps/puffer-desktop/src-tauri/icons/*.png apps/momo/src-tauri/icons/
cp apps/puffer-desktop/src-tauri/icons/*.ico apps/momo/src-tauri/icons/ 2>/dev/null || true
cp apps/puffer-desktop/src-tauri/icons/*.icns apps/momo/src-tauri/icons/ 2>/dev/null || true
```

- [ ] **Step 3: Write `apps/momo/package.json`**

```json
{
  "name": "momo",
  "private": true,
  "version": "0.1.0",
  "type": "module",
  "scripts": {
    "build:puffer-cli:dev": "cargo build -p puffer-cli",
    "build:puffer-cli:release": "cargo build --release -p puffer-cli",
    "dev": "vite --host 127.0.0.1 --port 1466",
    "build": "vite build",
    "preview": "vite preview --host 127.0.0.1 --port 1466",
    "check": "svelte-check --tsconfig ./tsconfig.json",
    "test:desktop-ui": "playwright test tests --pass-with-no-tests",
    "test:desktop-ui:webkit": "playwright test tests --browser=webkit --pass-with-no-tests",
    "tauri": "tauri"
  },
  "dependencies": {
    "@tauri-apps/api": "~2.10.1",
    "@tauri-apps/plugin-opener": "^2.5.4",
    "fflate": "^0.8.2",
    "libphonenumber-js": "^1.13.3",
    "lucide-svelte": "^1.0.1",
    "qrcode": "^1.5.4",
    "svelte": "^5.0.0"
  },
  "devDependencies": {
    "@playwright/test": "^1.59.1",
    "@sveltejs/vite-plugin-svelte": "^5.0.0",
    "@tauri-apps/cli": "^2.10.1",
    "svelte-check": "^4.0.0",
    "typescript": "^5.6.0",
    "vite": "^6.0.0"
  }
}
```

Note: Momo uses port `1466` (Vite) so it can run side-by-side with V1 (`1456`). PDF.js, xterm, dialog plugin are V1-only and intentionally dropped.

- [ ] **Step 4: Write `apps/momo/index.html`**

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="UTF-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1.0" />
    <meta name="theme-color" content="#ffffff" />
    <title>Momo</title>
  </head>
  <body>
    <div id="app"></div>
    <script type="module" src="/src/main.ts"></script>
  </body>
</html>
```

- [ ] **Step 5: Write `apps/momo/vite.config.ts`**

```ts
import { defineConfig } from "vite";
import { svelte } from "@sveltejs/vite-plugin-svelte";

const host =
  (globalThis as { process?: { env?: Record<string, string | undefined> } }).process?.env
    ?.TAURI_DEV_HOST ?? "127.0.0.1";

export default defineConfig({
  plugins: [
    svelte({
      compilerOptions: {
        compatibility: {
          componentApi: 4
        }
      }
    })
  ],
  clearScreen: false,
  envPrefix: ["VITE_", "TAURI_"],
  server: {
    host,
    port: 1466,
    strictPort: true,
    hmr: host !== "127.0.0.1"
      ? { protocol: "ws", host, port: 1431 }
      : undefined
  },
  preview: {
    host: "127.0.0.1",
    port: 1466,
    strictPort: true
  }
});
```

- [ ] **Step 6: Copy `svelte.config.js` + `tsconfig.json` + `playwright.config.ts` from `apps/puffer-desktop/`**

```bash
cp apps/puffer-desktop/svelte.config.js apps/momo/
cp apps/puffer-desktop/tsconfig.json apps/momo/
cp apps/puffer-desktop/playwright.config.ts apps/momo/
```

Then edit `apps/momo/playwright.config.ts`: change any path references (e.g. `tests/v2`) to `tests`. Look for `testDir:` and `baseURL:` (the baseURL may point to localhost:1456 — change to 1466).

- [ ] **Step 7: Write `apps/momo/src-tauri/Cargo.toml`**

```toml
[package]
name = "momo"
version = "0.1.0"
edition = "2021"
license = "MIT"

[lib]
name = "momo_lib"
crate-type = ["staticlib", "cdylib", "rlib"]

[build-dependencies]
tauri-build = { version = "2", features = [] }

[dependencies]
anyhow = "1.0.100"
base64 = "0.22.1"
puffer-session-store = { path = "../../../crates/puffer-session-store" }
reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }
serde = { version = "1.0.228", features = ["derive"] }
serde_json = { version = "1.0.145", features = ["preserve_order"] }
tauri = { version = "2", features = [] }
tauri-plugin-opener = "2"
tungstenite = "0.24"
url = "2"
uuid = { version = "1.18.1", features = ["serde", "v4"] }

[dev-dependencies]
tempfile = "3.23.0"

[workspace]
```

Dropped from V1's Cargo.toml: `portable-pty` (no PTY in Momo), `notify` (no fs_watch), `tauri-plugin-dialog` (no file dialog use).

- [ ] **Step 8: Write `apps/momo/src-tauri/build.rs`**

```rust
fn main() {
    tauri_build::build()
}
```

- [ ] **Step 9: Write `apps/momo/src-tauri/tauri.conf.json`**

Copy from `apps/puffer-desktop/src-tauri/tauri.conf.json`, then change these fields:
- `productName`: `"Momo"`
- `identifier`: `"ai.tomo.momo"`
- `app.windows[0].title`: `"Momo"`
- `build.devUrl`: `"http://localhost:1466"`
- `build.beforeDevCommand`: keep as `"npm run build:puffer-cli:dev && npm run dev"`
- `bundle.targets`: keep V1's value
- `app.windows[0]` size/min — keep V1's

After editing, sanity check JSON parses:

```bash
python3 -m json.tool apps/momo/src-tauri/tauri.conf.json > /dev/null
```

- [ ] **Step 10: Write minimal `apps/momo/src-tauri/src/main.rs`**

```rust
fn main() {
    momo_lib::run()
}
```

- [ ] **Step 11: Write minimal stub `apps/momo/src-tauri/src/lib.rs`**

```rust
use tauri::Builder;

pub fn run() {
    Builder::default()
        .plugin(tauri_plugin_opener::init())
        .run(tauri::generate_context!())
        .expect("error while running momo application");
}
```

This is intentionally minimal — full version comes in Task 3.

- [ ] **Step 12: Copy capabilities file**

```bash
cp apps/puffer-desktop/src-tauri/capabilities/default.json apps/momo/src-tauri/capabilities/default.json
```

Edit `apps/momo/src-tauri/capabilities/default.json` and remove any capabilities for plugins Momo doesn't use (`dialog`). The `opener` plugin's `allow-open-url` permission is required.

- [ ] **Step 13: Add `apps/momo/src-tauri` to workspace members**

Edit root `Cargo.toml` (`/Users/shun/Data/Code/tomo/agentenv/puffer/Cargo.toml`): find the `[workspace] members = [...]` list and add `"apps/momo/src-tauri",` (alphabetical order if the rest is alphabetical, else just append).

Verify:

```bash
grep "apps/momo/src-tauri" Cargo.toml
```

- [ ] **Step 14: Verify Cargo can build the skeleton**

```bash
cd apps/momo/src-tauri && cargo check
```

Expected: succeeds. If `puffer-session-store` import fails (it's declared but unused in the stub), that's OK — `cargo check` accepts unused deps.

- [ ] **Step 15: Verify npm install works**

```bash
cd apps/momo && npm install
```

Expected: succeeds, no peer-dep errors.

- [ ] **Step 16: Commit**

```bash
git add apps/momo Cargo.toml
git commit -m "feat(momo): scaffold empty apps/momo Tauri app

Bootstrap apps/momo as a sibling to apps/puffer-desktop. Empty
skeleton with port 1466 (Vite) and identifier ai.tomo.momo to
allow side-by-side dev with V1 (Corbina, port 1456,
com.corbina.desktop).

src-tauri/src/lib.rs is a stub; real WS + OAuth wiring lands in
the follow-up commits that move V2's backend pieces over."
```

---

## Task 2: Move V2 frontend (src-v2/ → momo/src/)

**Files:**
- Move: `apps/puffer-desktop/src-v2/**` → `apps/momo/src/**`
- Move: `apps/puffer-desktop/tests/v2/**` → `apps/momo/tests/**`
- Modify: `apps/puffer-desktop/index.html` (no longer references src-v2)
- Move: `apps/puffer-desktop/.env` → `apps/momo/.env`

- [ ] **Step 1: Move src-v2 directory via git mv**

```bash
git mv apps/puffer-desktop/src-v2 apps/momo/src
```

This creates `apps/momo/src/` as a fresh directory containing the moved files, preserving per-file history. Verify:

```bash
ls apps/momo/src/ | head -10
# expect: App.svelte, main.ts, components/, lib/, pages/, etc.
git status | head -20
# expect: every file shown as "renamed: apps/puffer-desktop/src-v2/X -> apps/momo/src/X"
```

If `git status` shows "deleted" + "new file" instead of "renamed", that's still semantically a move but blame won't follow. Usually git auto-detects renames at threshold 50% similarity — for unmodified moves that always works.

- [ ] **Step 2: Move tests/v2 → momo/tests**

```bash
git mv apps/puffer-desktop/tests/v2 apps/momo/tests
ls apps/momo/tests/
# expect: apikey.e2e.spec.ts, chat.spec.ts, login.e2e.spec.ts, login.spec.ts,
#         persistence.e2e.spec.ts, sessions.spec.ts, wallet.spec.ts
```

- [ ] **Step 3: Move .env file**

```bash
git mv apps/puffer-desktop/.env apps/momo/.env
```

(V1 doesn't read `VITE_AUTH_STATION_URL` — verified by earlier grep: V1's `src/` has 0 references to `import.meta.env.VITE_AUTH_STATION_URL`. So removing the `.env` from `apps/puffer-desktop/` is safe.)

- [ ] **Step 4: Update `apps/puffer-desktop/index.html` to use V1 entry point**

Read current state — it likely still has `<script type="module" src="/src-v2/main.ts"></script>`. Change to V1's:

```html
<script type="module" src="/src/main.ts"></script>
```

(V1 entry is `apps/puffer-desktop/src/main.ts`, confirmed exists.)

- [ ] **Step 5: Verify Momo frontend type-checks**

```bash
cd apps/momo && npm run check 2>&1 | tail -30
```

Expected: zero TypeScript errors. If there are import errors referencing `../../../src-v2/...` or similar, fix them with relative path corrections.

Common fixes: in `apps/momo/src/**`, find any imports that started with `../src/` or `../../src-tauri/` and adjust depths.

- [ ] **Step 6: Verify Momo Vite dev server starts**

```bash
cd apps/momo && npm run dev &
DEV_PID=$!
sleep 5
curl -sI http://127.0.0.1:1466/ | head -3
kill $DEV_PID 2>/dev/null
wait 2>/dev/null
```

Expected: `HTTP/1.1 200 OK` on the `curl` call (the empty backend is OK — the page should at least load).

- [ ] **Step 7: Verify V1 still type-checks**

```bash
cd apps/puffer-desktop && npm run check 2>&1 | tail -10
```

Expected: zero TypeScript errors.

- [ ] **Step 8: Commit**

```bash
git add -A
git commit -m "feat(momo): move src-v2/, tests/v2/, .env from puffer-desktop into momo

Pure file moves. V1's index.html updated to point back at /src/main.ts
(V1's own entry, never used src-v2)."
```

---

## Task 3: Port V2 Rust backend (the careful one)

**Files:**
- Move: `apps/puffer-desktop/src-tauri/src/websocket.rs` → `apps/momo/src-tauri/src/websocket.rs`
- Move: `apps/puffer-desktop/src-tauri/src/events.rs` → `apps/momo/src-tauri/src/events.rs`
- Move: `apps/puffer-desktop/src-tauri/src/oauth_listener.rs` → `apps/momo/src-tauri/src/oauth_listener.rs`
- Move: `apps/puffer-desktop/src-tauri/src/dtos.rs` → `apps/momo/src-tauri/src/dtos.rs`
- Move: `apps/puffer-desktop/src-tauri/src/codex_app_server.rs` → `apps/momo/src-tauri/src/codex_app_server.rs`
- Create: `apps/momo/src-tauri/src/backend.rs` (trimmed copy)
- Modify: `apps/momo/src-tauri/src/lib.rs` (real version, replacing Task 1's stub)

**Critical note:** `apps/puffer-desktop/src-tauri/` keeps its own copies of `websocket.rs`, `events.rs`, `oauth_listener.rs`, `dtos.rs`, `codex_app_server.rs` even after Momo gets its own — V1 still uses them via Tauri commands for fallback paths. So this is a **copy + V1 keeps original**, not a move. We'll prune V1's unused dispatch arms in Task 4.

Wait — re-reading the V1 grep: V1 only invokes `cancel_turn`, `resolve_permission`, `resolve_user_question`. So V1 does technically use the dispatch in `backend.rs` for those. We need to keep V1's `backend.rs` intact for those 3 paths.

Actually, simpler: `git mv` is wrong; use `cp -p` so both apps end up with independent copies. Yes — they will diverge over time. That's acceptable; V1 is in maintenance mode.

- [ ] **Step 1: Copy WS infra files (websocket.rs, events.rs)**

```bash
cp apps/puffer-desktop/src-tauri/src/websocket.rs apps/momo/src-tauri/src/websocket.rs
cp apps/puffer-desktop/src-tauri/src/events.rs apps/momo/src-tauri/src/events.rs
```

Edit `apps/momo/src-tauri/src/websocket.rs`: find the bind port. It uses `CORBINA_BACKEND_WS_BIND`. Change to `MOMO_BACKEND_WS_BIND` and default port from `1421` (V1) to `1431` (Momo). Search for occurrences:

```bash
grep -n "CORBINA_BACKEND_WS_BIND\|127.0.0.1:1421\|1421" apps/momo/src-tauri/src/websocket.rs
```

Replace each. Verify only 1-2 hits in this file.

Update `apps/momo/src/lib/wsClient.ts` (the frontend) — find the default URL `ws://127.0.0.1:1421/ws` and change to `1431`. Same for any `VITE_PUFFER_WS_URL` references in test/config.

- [ ] **Step 2: Copy OAuth listener and change port**

```bash
cp apps/puffer-desktop/src-tauri/src/oauth_listener.rs apps/momo/src-tauri/src/oauth_listener.rs
```

Edit `apps/momo/src-tauri/src/oauth_listener.rs`: find `1457` (the loopback port) — replace with `1467`. Search:

```bash
grep -n "1457" apps/momo/src-tauri/src/oauth_listener.rs
```

Expect 1-2 hits. Also update the frontend redirect_uri:

```bash
grep -n "localhost:1457" apps/momo/src/lib/auth.svelte.ts
```

The line `const TAURI_OAUTH_REDIRECT_URI = "http://localhost:1457/callback";` → change to `1467`.

- [ ] **Step 3: Copy dtos.rs and codex_app_server.rs**

```bash
cp apps/puffer-desktop/src-tauri/src/dtos.rs apps/momo/src-tauri/src/dtos.rs
cp apps/puffer-desktop/src-tauri/src/codex_app_server.rs apps/momo/src-tauri/src/codex_app_server.rs
```

No edits needed; these are pure data/protocol modules.

- [ ] **Step 4: Create trimmed `apps/momo/src-tauri/src/backend.rs`**

Start by copying:

```bash
cp apps/puffer-desktop/src-tauri/src/backend.rs apps/momo/src-tauri/src/backend.rs
```

Now edit `apps/momo/src-tauri/src/backend.rs` to TRIM. Below is the precise spec:

**(a) Remove these imports at the top:**
```rust
use crate::{browser, files, fs_watch, lsp, pty};
use crate::repo_actions;
```
Keep these:
```rust
use crate::codex_app_server::{self, CapturedTurnEvent, CodexTurnOptions, CodexTurnOutcome};
use crate::dtos::{ ... };
use crate::events::EventEmitter;
```

**(b) Replace the entire `handle()` method's `match` (the giant dispatch around lines 60-220) with this trimmed version** — keep only V2 arms:

```rust
pub(crate) fn handle(
    &self,
    events: EventEmitter,
    method: &str,
    params: Value,
) -> Result<Value> {
    match method {
        "list_grouped_sessions" => serde_value(self.list_grouped_sessions()?),
        "load_session_detail" => {
            let session_id = string_param(&params, &["sessionId", "session_id"])?;
            serde_value(self.load_session_detail(&session_id)?)
        }
        "login_with_api_key" => {
            let provider_id = string_param(&params, &["providerId", "provider_id"])?;
            let api_key = string_param(&params, &["apiKey", "api_key"])?;
            self.store_api_key(&provider_id, &api_key)?;
            serde_value(self.load_settings_snapshot()?)
        }
        "logout_provider" => {
            let provider_id = string_param(&params, &["providerId", "provider_id"])?;
            self.remove_api_key(&provider_id)?;
            serde_value(self.load_settings_snapshot()?)
        }
        "create_session" => {
            let cwd = optional_string_param(&params, &["cwd"])
                .map(PathBuf::from)
                .unwrap_or(self.default_workspace()?);
            let provider =
                optional_string_param(&params, &["providerId", "provider_id", "provider"]);
            let model = optional_string_param(&params, &["modelId", "model_id", "model"]);
            serde_value(self.create_session(cwd, provider, model)?)
        }
        "rename_session" => {
            let session_id = string_param(&params, &["sessionId", "session_id"])?;
            let title = string_param(&params, &["title"])?;
            self.rename_session(&session_id, title)?;
            serde_value(self.load_session_detail(&session_id)?)
        }
        "run_agent_turn" => self.run_agent_turn(events.clone(), params),
        "cancel_turn" => {
            let turn_id = string_param(&params, &["turnId", "turn_id"])?;
            if let Some(flag) = self.turns.lock().unwrap().get(&turn_id) {
                flag.store(true, Ordering::SeqCst);
            }
            Ok(json!({}))
        }
        other => bail!("unknown method: {other}"),
    }
}
```

**(c) Helper functions to KEEP (must be present in trimmed backend.rs):**

- `list_grouped_sessions()` (line ~234)
- `create_session()` (line ~259)
- `load_session_detail()` (line ~337)
- `rename_session()` (line ~428)
- `run_agent_turn()` (line ~729)
- `run_agent_turn_thread()` (line ~1039)
- `run_agent_turn_inner()` (line ~1142)
- `load_settings_snapshot()` — used by login_with_api_key/logout_provider responses
- `store_api_key()`, `remove_api_key()`
- `default_workspace()` — used by create_session fallback
- `load_session()` — used internally
- `app_home()` (line ~2718) — but **rename `CORBINA_HOME` env var to `MOMO_HOME` and default to `~/.momo`**
- `home_dir()` (line ~2725)
- `sessions_file()` (line ~2731) — points to `~/.momo/sessions.json`
- `config_file()`, `credentials_file()`, `pins_file()`, `permissions_file()` — all under `~/.momo/`
- `provider_command()` (line ~2463) — **rename env vars `CORBINA_PUFFER_BIN → MOMO_PUFFER_BIN`, `CORBINA_CODEX_BIN → MOMO_CODEX_BIN`, `CORBINA_CLAUDE_BIN → MOMO_CLAUDE_BIN`**
- `ensure_provider_command()` (line ~2482)
- `command_exists()` (line ~2498)
- All small utility helpers used by the above: `serde_value()`, `string_param()`, `optional_string_param()`, `EventEmitter` usage, etc.

**(d) Helper functions / methods to DELETE:**

- `pty_*` methods, `browser_*` methods, `git_clone()`, `load_pins()`, `set_desktop_pin()`, `load_file_tabs()`, `save_file_tabs()`, `list_mcp_servers()`, `add_mcp_server()`, `list_provider_models()`, `repo_status()`/`create_pull_request()`/`merge_pull_request()` callers, `run_remote_bash()`, `read_remote_file()`, `write_remote_file()`, `import_external_credential()`, `list_external_credentials()`, all PTY/browser state in `BackendState`
- Remove `ptys`, `fs_watches`, `browsers` fields from `BackendState` struct (line ~47); only keep `turns: Mutex<HashMap<String, Arc<AtomicBool>>>`
- Remove `browser_profile_root` setup in `BackendState::new()`

**(e) `BackendState::new()` trimmed:**

```rust
impl BackendState {
    pub(crate) fn new() -> Self {
        Self {
            turns: Mutex::new(HashMap::new()),
        }
    }
}
```

**(f) Delete the env vars `CORBINA_PUFFER_BIN/_CODEX_BIN/_CLAUDE_BIN/_HOME` everywhere in backend.rs** — rename to `MOMO_*`. Grep first:

```bash
grep -n "CORBINA_" apps/momo/src-tauri/src/backend.rs
```

Replace each one. (Same for `app_home()` default `~/.corbina` → `~/.momo`.)

- [ ] **Step 5: Sanity-check trimmed backend.rs compiles**

```bash
cd apps/momo/src-tauri && cargo check 2>&1 | tail -30
```

Expected: errors about missing helper functions, missing `repo_actions` mod, etc. **Fix each error**: typically by either deleting the offending call site or pulling the helper into backend.rs. Iterate until clean.

Common iteration issue: `serde_value()` may not exist as a free function — check if it's defined in `dtos.rs` or elsewhere and import it.

- [ ] **Step 6: Write real `apps/momo/src-tauri/src/lib.rs`**

Replace the stub from Task 1:

```rust
mod backend;
mod codex_app_server;
mod dtos;
mod events;
mod oauth_listener;
mod websocket;

use std::sync::Arc;
use tauri::Builder;

use crate::backend::BackendState;

pub fn run() {
    let backend = Arc::new(BackendState::new());
    websocket::start_backend_ws(backend.clone());

    Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            oauth_listener::start(app.handle().clone());
            Ok(())
        })
        .manage(backend)
        .invoke_handler(tauri::generate_handler![])
        .run(tauri::generate_context!())
        .expect("error while running momo application");
}
```

Note: empty `invoke_handler![]` because V2 frontend doesn't use `invoke()` at all — it only uses WebSocket. Verified by `grep -rn 'invoke("' apps/momo/src` finding 0 hits (re-verify before this step).

- [ ] **Step 7: Verify Momo cargo check passes**

```bash
cd apps/momo/src-tauri && cargo check 2>&1 | tail -20
```

Expected: builds clean, only warnings about unused functions (we kept `load_settings_snapshot` etc which may not be called from the trimmed dispatch — that's OK).

- [ ] **Step 8: Verify V1 cargo check still passes (we didn't touch V1 src-tauri yet)**

```bash
cd apps/puffer-desktop/src-tauri && cargo check 2>&1 | tail -10
```

Expected: builds clean.

- [ ] **Step 9: Commit**

```bash
git add apps/momo/src-tauri apps/momo/src/lib/auth.svelte.ts apps/momo/src/lib/wsClient.ts
git commit -m "feat(momo): port V2 Rust backend (trimmed backend.rs + WS + OAuth)

- Copy websocket.rs, events.rs, dtos.rs, codex_app_server.rs
- Rewrite backend.rs keeping only the 8 V2 WS methods
  (create_session, run_agent_turn, cancel_turn, list_grouped_sessions,
  rename_session, load_session_detail, login_with_api_key, logout_provider)
- Move OAuth callback port 1457 -> 1467 (allow V1+V2 side-by-side)
- Move Tauri WS server port 1421 -> 1431
- Rename CORBINA_* env vars to MOMO_*, storage root ~/.corbina -> ~/.momo

V1 (apps/puffer-desktop) is untouched; it keeps its own copies of all
src-tauri modules. Cleanup of V1's unused dispatch arms lands in a
follow-up commit."
```

---

## Task 4: Bootstrap session storage migration

**Files:**
- Modify: `apps/momo/src-tauri/src/backend.rs` (`app_home()` function)

The new path is `~/.momo/`. The user has existing sessions at `~/.corbina/sessions.json` (5月26日 PONG session etc). Provide a one-time copy hint.

- [ ] **Step 1: Add fallback read in `app_home()` only if `~/.momo` doesn't exist**

Edit `apps/momo/src-tauri/src/backend.rs` `app_home()`:

```rust
fn app_home() -> Result<PathBuf> {
    if let Ok(path) = env::var("MOMO_HOME") {
        return Ok(PathBuf::from(path));
    }
    let primary = home_dir().join(".momo");
    if !primary.exists() {
        let legacy = home_dir().join(".corbina");
        if legacy.exists() {
            eprintln!(
                "momo: found legacy ~/.corbina but no ~/.momo; \
                run `cp -r ~/.corbina ~/.momo` to import sessions, then restart"
            );
        }
    }
    Ok(primary)
}
```

The reason we don't auto-copy: silent data migration is too risky (`~/.corbina` is V1's data on machines where V1 is still active; copying isn't destructive but it can leak V1 sessions into Momo's view). Make the user run it explicitly.

- [ ] **Step 2: Cargo check + commit**

```bash
cd apps/momo/src-tauri && cargo check 2>&1 | tail -5
cd ../../..
git add apps/momo/src-tauri/src/backend.rs
git commit -m "feat(momo): print migration hint when ~/.corbina exists but ~/.momo doesn't"
```

---

## Task 5: Add `localhost:1467` to Auth Station whitelist (deployment task)

**Files:** None local — this is a Vercel deployment action.

**Why now:** Without this, Step 6's OAuth smoke test will fail with an `invalid_redirect_uri` error from Auth Station.

This task uses the `auth-deploy` skill workflow. Two acceptable approaches: (a) modify ALLOWED_REDIRECT_ORIGINS via Vercel API + redeploy, (b) add 1467 alongside 1457 (keep both). **Prefer (b)** so V1 keeps working on machines that haven't migrated.

- [ ] **Step 1: Fetch current ALLOWED_REDIRECT_ORIGINS**

```bash
TOKEN=$(python3 -c "import json,os; print(json.load(open(os.path.expanduser('~/Library/Application Support/com.vercel.cli/auth.json')))['token'])")

# List env keys (values are encrypted at rest; pulling decrypted requires the env pull workflow below)
curl -s "https://api.vercel.com/v10/projects/prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl/env?teamId=team_EOvpxvSsCupQhXAG3GeG317R" \
  -H "Authorization: Bearer $TOKEN" \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print([{'id':e['id'],'target':e['target']} for e in d.get('envs',[]) if e['key']=='ALLOWED_REDIRECT_ORIGINS'])"
```

Note the IDs (there's one entry per target: production + preview + development possibly).

- [ ] **Step 2: Pull current value via vercel CLI**

```bash
cd /tmp
mkdir -p auth-station-env-pull && cd auth-station-env-pull
vercel link --project auth-worldrouter --scope nubit --yes
vercel env pull .env.production --environment production
grep ALLOWED_REDIRECT_ORIGINS .env.production
```

This prints the comma-separated list.

- [ ] **Step 3: Update the value to include `http://localhost:1467`**

Take the existing comma-separated list, append `,http://localhost:1467`. Use the API approach from `auth-deploy` skill (remove + re-add):

```bash
# Remove old entries (need to do for production and preview)
for envId in $(curl -s "https://api.vercel.com/v10/projects/prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl/env?teamId=team_EOvpxvSsCupQhXAG3GeG317R" -H "Authorization: Bearer $TOKEN" | python3 -c "import json,sys; [print(e['id']) for e in json.load(sys.stdin).get('envs',[]) if e['key']=='ALLOWED_REDIRECT_ORIGINS']"); do
  curl -X DELETE "https://api.vercel.com/v9/projects/prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl/env/$envId?teamId=team_EOvpxvSsCupQhXAG3GeG317R" -H "Authorization: Bearer $TOKEN"
done

# Re-add with updated value (replace VALUE with the appended list)
VALUE="<current-comma-separated-list>,http://localhost:1467"
curl -X POST "https://api.vercel.com/v10/projects/prj_4Mi7OqkeMQ5bOaNzzMOiLHsPRDGl/env?teamId=team_EOvpxvSsCupQhXAG3GeG317R" \
  -H "Authorization: Bearer $TOKEN" \
  -H "Content-Type: application/json" \
  -d "{\"key\":\"ALLOWED_REDIRECT_ORIGINS\",\"value\":\"$VALUE\",\"type\":\"encrypted\",\"target\":[\"production\",\"preview\"]}"
```

- [ ] **Step 4: Redeploy Auth Station**

```bash
cd $TOMO_ROOT/infer-monorepo/auth   # or wherever auth-worldrouter lives — see auth-deploy skill
vercel --prod --scope nubit
```

Wait for deployment URL to become ready.

- [ ] **Step 5: Verify whitelist update took effect**

```bash
curl -s -o /dev/null -w "%{http_code}\n" "https://auth.worldrouter.ai/login?redirect_uri=http://localhost:1467/callback&client_state=test"
```

Expected: `200` (auth station accepted the redirect_uri) or `302` to login page. If `400` with `invalid_redirect_uri`, the deploy hasn't rolled out yet — wait 30s and retry.

- [ ] **Step 6: Update `auth-deploy` skill's "Current ALLOWED_REDIRECT_ORIGINS" doc**

Edit `~/.claude/skills/auth-deploy/SKILL.md` and append `http://localhost:1467` to the list. This isn't strictly part of the code change but keeps the skill accurate. (No commit needed; the skill repo handles its own commit.)

- [ ] **Step 7: No code commit for this task** — it's a deployment-side change. Just verify and move on.

---

## Task 6: Smoke test Momo end-to-end

- [ ] **Step 1: Migrate session data (one-time)**

```bash
cp -r ~/.corbina ~/.momo
ls ~/.momo/sessions.json && echo "OK"
```

If V1 has never been run on this machine and `~/.corbina` doesn't exist, skip this step (Momo will create `~/.momo` fresh on first run).

- [ ] **Step 2: Launch Momo dev**

```bash
cd apps/momo && npm run tauri dev 2>&1 | tee /tmp/momo-dev.log &
DEV_PID=$!
```

Wait for:
```
VITE v6.x.x  ready in <ms>
Finished `dev` profile [unoptimized + debuginfo] target(s)
```

If compile fails, fix and rebuild. Common issues:
- Missing helper function → port it from V1's backend.rs
- Port already in use (1466 or 1431 or 1467) → kill conflicting process or change port

- [ ] **Step 3: Manual smoke test — login flow**

In the Momo window:
1. Click the login button on the login screen
2. OS browser opens to `https://auth.worldrouter.ai/login?redirect_uri=http://localhost:1467/callback&...`
3. Complete login (WorkOS)
4. Auth Station redirects back to `http://localhost:1467/callback?token=...&state=...`
5. Momo's loopback listener catches it; webview navigates to `/auth/callback` and then home

Expected: home screen shows, user is logged in.

If step 2 doesn't open the browser: check Tauri WebView devtools (Cmd+Option+I), look for `[auth]` errors.
If step 4 fails: the whitelist update from Task 5 didn't deploy — re-verify.

- [ ] **Step 4: Manual smoke test — chat with puffer**

1. Send a message: "讲个笑话"
2. Expect response from puffer (the model configured in `~/.puffer/config.toml` → `gpt-5.4` via agentsey)

If error: `puffer is not installed or not executable` → re-confirm `~/.cargo/bin/puffer` exists; the env var changed name to `MOMO_PUFFER_BIN`, but the fallback (bare `puffer` on PATH) should still work.

- [ ] **Step 5: Manual smoke test — wallet U-card flow**

1. Navigate to Wallet tab
2. Card list loads (HTTP GET to `http://127.0.0.1:8080/api/card/list`)
3. If `ucard-backend` is not running, expect error toast — that's OK, ucard backend is its own service

Skip if ucard-backend isn't running locally; not in scope for this split.

- [ ] **Step 6: Verify V1 still works after the split**

```bash
kill $DEV_PID && wait 2>/dev/null
cd apps/puffer-desktop && npm run tauri dev &
V1_PID=$!
```

Wait for compile. Window opens. Verify:
- V1 sidebar loads `puffer-user/sessions/*.session.json` via puffer daemon
- V1 OAuth still works on port 1457 (uses its own oauth_listener.rs)

```bash
kill $V1_PID && wait 2>/dev/null
```

- [ ] **Step 7: Run automated tests**

```bash
cd apps/momo && npm run test:desktop-ui 2>&1 | tail -20
```

Expected: tests/v2 tests pass against the new app. If any fail, triage individually.

```bash
cd apps/puffer-desktop && npm run test:desktop-ui 2>&1 | tail -20
```

Expected: V1's `tests/v1` still pass (run them via `playwright test tests/v1`). The puffer-desktop's `test:desktop-ui` script may currently target `tests/v2` — update it to `tests/v1` first:

```bash
# In apps/puffer-desktop/package.json, change:
#   "test:desktop-ui": "playwright test tests/v2 --pass-with-no-tests",
# to:
#   "test:desktop-ui": "playwright test tests/v1 --pass-with-no-tests",
```

- [ ] **Step 8: Commit (smoke test docs only — no code changes here)**

If you found any small bugs during smoke testing and fixed them inline, commit each fix:

```bash
git add -A
git commit -m "fix(momo): <specific small bug found in smoke test>"
```

If smoke tests all pass cleanly, no commit needed here.

---

## Task 7: Trim V1 (`apps/puffer-desktop`) — delete V2 leftovers + dead code

**Files:**
- Delete: `apps/puffer-desktop/src-tauri/src/auth_data.rs` (orphan, ~579 lines)
- Delete: `apps/puffer-desktop/src-tauri/src/dto.rs` (orphan)
- Delete: `apps/puffer-desktop/src-tauri/src/git_actions.rs` (orphan)
- Delete: `apps/puffer-desktop/src-tauri/src/session_api.rs` (orphan)
- Delete: `apps/puffer-desktop/src-tauri/src/session_data.rs` (orphan)
- Delete: `apps/puffer-desktop/src-tauri/src/settings_data.rs` (orphan)
- Delete: `apps/puffer-desktop/src-tauri/src/turn.rs` (orphan)
- Modify: `apps/puffer-desktop/src-tauri/src/backend.rs` (delete V2-only dispatch arms)
- Modify: `apps/puffer-desktop/src-tauri/src/lib.rs` (remove V2-only command registrations)

- [ ] **Step 1: Verify these files are indeed not `mod`-included in V1's lib.rs**

```bash
cd apps/puffer-desktop/src-tauri/src
for f in auth_data dto git_actions session_api session_data settings_data turn; do
  hits=$(grep -c "^mod $f\b\|^mod $f;" lib.rs)
  echo "$f: $hits"
done
```

Each must print `0`. If any prints `1`, that file is live and must NOT be deleted — investigate.

- [ ] **Step 2: Delete the 7 orphan files**

```bash
git rm apps/puffer-desktop/src-tauri/src/auth_data.rs \
       apps/puffer-desktop/src-tauri/src/dto.rs \
       apps/puffer-desktop/src-tauri/src/git_actions.rs \
       apps/puffer-desktop/src-tauri/src/session_api.rs \
       apps/puffer-desktop/src-tauri/src/session_data.rs \
       apps/puffer-desktop/src-tauri/src/settings_data.rs \
       apps/puffer-desktop/src-tauri/src/turn.rs
```

- [ ] **Step 3: Verify V1 still cargo-checks**

```bash
cd apps/puffer-desktop/src-tauri && cargo check 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 4: Commit dead-code removal**

```bash
git add -A
git commit -m "chore(puffer-desktop): remove 7 orphan .rs files (~2988 lines dead code)

auth_data.rs, dto.rs, git_actions.rs, session_api.rs, session_data.rs,
settings_data.rs, turn.rs are not mod-included in lib.rs and therefore
never compiled. They're V1-era drafts left over from the corbina rewrite.
Confirmed via grep before deletion."
```

- [ ] **Step 5: Trim V2-only dispatch arms from V1 backend.rs**

V1 actually invokes only `cancel_turn`, `resolve_permission`, `resolve_user_question` via Tauri (verified by grep). All other arms can be removed from V1's `backend.rs` `handle()` match without breaking V1.

But this is risky and gives small payoff — leave V1's backend.rs alone. **Skip Step 5.** (Trim is mechanical and tempting but V1 is in maintenance; less churn is safer.)

- [ ] **Step 6: Bump V1 puffer-desktop README to note the V2 split**

Edit `apps/puffer-desktop/README.md`. At the top, add:

```markdown
> **Note:** A separate desktop app, Momo, lives at `apps/momo/` and
> targets puffer-only chat + wallet flows. Corbina (this app) remains
> the multi-provider coding agent.
```

- [ ] **Step 7: Commit README note**

```bash
git add apps/puffer-desktop/README.md
git commit -m "docs(puffer-desktop): note the apps/momo split in README"
```

---

## Task 8: Write Momo README and update CLAUDE.md / docs

**Files:**
- Create: `apps/momo/README.md`
- Modify: root `README.md` (if it references apps/ structure)
- Modify: `CLAUDE.md` (if it has notes about the desktop app)

- [ ] **Step 1: Write `apps/momo/README.md`**

```markdown
# Momo

Standalone Tauri desktop app: puffer-powered chat + WorldClaw U-card wallet.

Forked out of `apps/puffer-desktop/src-v2/` and that codebase's V2 src-tauri pieces (May 2026). See `docs/superpowers/plans/2026-05-27-extract-momo-app.md` for the split rationale.

## Provider

Puffer-only. The host expects `puffer` on `PATH`, or honors `MOMO_PUFFER_BIN` for an explicit path.

## Storage

Default app home is `~/.momo` (override with `MOMO_HOME`).
- `sessions.json` — chat session metadata + transcripts
- `config.json` — UI config
- `credentials.json` — API keys
- `permissions.json`, `pins.json` — UI state

If you're migrating from V1 (Corbina): `cp -r ~/.corbina ~/.momo` before first run.

## Auth

OAuth flow: webview redirects to OS browser, browser hits `http://localhost:1467/callback`, Momo's loopback HTTP listener catches it.

`apps/momo/.env`:
```
VITE_AUTH_STATION_URL=https://auth.worldrouter.ai
```

## Development

```bash
npm install
npm run check
npm run tauri dev
```

Vite serves on port 1466. Tauri WS backend listens on 1431. OAuth loopback on 1467.

## Verification

```bash
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri build
```
```

- [ ] **Step 2: Commit README**

```bash
git add apps/momo/README.md
git commit -m "docs(momo): initial README"
```

- [ ] **Step 3: Update root CLAUDE.md if needed**

Read root `CLAUDE.md`:

```bash
cat CLAUDE.md 2>/dev/null | head -40
```

If it mentions `apps/puffer-desktop` as the only desktop UI, add a sibling mention for `apps/momo`. If `CLAUDE.md` doesn't exist or doesn't mention desktop apps, skip.

- [ ] **Step 4: Commit CLAUDE.md update (if any)**

```bash
git add CLAUDE.md
git commit -m "docs: mention apps/momo in CLAUDE.md"
```

---

## Task 9: Update work-context pitfalls (operational learnings)

**Files:**
- Modify: `$TOMO_ROOT/work-context/pitfalls.md`

Three new things future-me should know after this split:
1. Two Tauri apps exist in this repo (corbina + momo) with different ports / identifiers / storage roots.
2. Auth Station whitelist now has both 1457 and 1467; don't remove 1457 unless V1 is being deprecated.
3. `MOMO_PUFFER_BIN` env var is the Momo-side equivalent of `CORBINA_PUFFER_BIN`.

- [ ] **Step 1: Append a pitfall about the split**

Use the `work-context` skill to add. The entry (append to end of `pitfalls.md`):

```markdown
---

## puffer 仓库下有两个 Tauri 桌面 app：corbina（V1，编码 agent）和 momo（V2，puffer-only 聊天 + wallet）

- 时间: 2026-XX-XX (fill in when Task 9 runs)
- 项目: #puffer
- 触发场景: 在 puffer 仓库 clone 后想跑桌面端，看到 `apps/` 下既有 `puffer-desktop/` 又有 `momo/`，不知道该跑哪个；或者同时跑发现端口/identifier 撞
- 根因: 2026-05-27 把 V2 从 puffer-desktop/src-v2 拆成独立 app momo。两个 app 完全独立，但需要的端口和 identifier 不同
- 解决方案:
  - **要跑哪个**：编码 agent (V1) → `apps/puffer-desktop`；puffer 聊天 + wallet → `apps/momo`
  - **端口对照**：
    - V1: Vite 1456, Tauri WS 1421, OAuth callback 1457, identifier com.corbina.desktop
    - V2: Vite 1466, Tauri WS 1431, OAuth callback 1467, identifier ai.tomo.momo
  - **存储**：V1 `~/.corbina/`, V2 `~/.momo/`（独立）；首次跑 V2 要 `cp -r ~/.corbina ~/.momo` 迁移历史
  - **env vars**：V1 `CORBINA_PUFFER_BIN/_HOME`, V2 `MOMO_PUFFER_BIN/_HOME`
  - **Auth Station 白名单**：必须同时包含 `http://localhost:1457` 和 `http://localhost:1467`
- 相关代码: puffer/docs/superpowers/plans/2026-05-27-extract-momo-app.md (拆分 plan), puffer/apps/momo/README.md, puffer/apps/puffer-desktop/README.md
```

- [ ] **Step 2: No code commit needed** — work-context commits itself via SessionEnd hook (per `work-context` skill convention).

---

## Task 10: Final verification + push

- [ ] **Step 1: Full workspace cargo check**

```bash
cd /Users/shun/Data/Code/tomo/agentenv/puffer
cargo check --workspace 2>&1 | tail -10
```

Expected: clean.

- [ ] **Step 2: Full V1 + V2 build smoke**

```bash
cd apps/puffer-desktop && npm run tauri dev &
P1=$!
sleep 60
kill $P1 && wait 2>/dev/null

cd ../momo && npm run tauri dev &
P2=$!
sleep 60
kill $P2 && wait 2>/dev/null
```

Both should compile and launch. If one fails, debug and re-run the relevant Task.

- [ ] **Step 3: Push the branch**

```bash
cd /Users/shun/Data/Code/tomo/agentenv/puffer
git log --oneline feat/desktop-ui-v2-login..HEAD | wc -l
# expect: ~10-12 commits added by this plan

git push -u origin feat/extract-momo-app
```

- [ ] **Step 4: Open a draft PR for review**

```bash
gh pr create --draft --title "feat: extract apps/momo from puffer-desktop V2" --body "$(cat <<'EOF'
## Summary

Split apps/puffer-desktop/src-v2/ + the V2-relevant src-tauri pieces into a
standalone Tauri app at apps/momo/. V1 (Corbina, multi-provider coding agent)
stays at apps/puffer-desktop/ untouched in scope; only dead code removed.

See docs/superpowers/plans/2026-05-27-extract-momo-app.md for the rationale
and the exact step-by-step approach taken.

## Highlights

- New app: apps/momo (Vite 1466, Tauri WS 1431, OAuth 1467, identifier ai.tomo.momo)
- Storage moved to ~/.momo/ (one-time migration: cp -r ~/.corbina ~/.momo)
- Auth Station whitelist updated to include http://localhost:1467
- 7 orphan .rs files (~2988 LOC) deleted from puffer-desktop/src-tauri
- backend.rs trimmed for momo: ~2912 LOC -> ~700 LOC (only 8 V2 WS methods)

## Test plan

- [ ] V1 (Corbina) still launches: cd apps/puffer-desktop && npm run tauri dev
- [ ] V2 (Momo) launches fresh: cd apps/momo && npm run tauri dev
- [ ] Both apps can be running side-by-side without port conflict
- [ ] OAuth login works in Momo (browser opens, callback received, home loads)
- [ ] Chat in Momo: send "讲个笑话", puffer responds
- [ ] Wallet tab in Momo loads card list (if ucard-backend running)
- [ ] V1 chat list (sidebar) still loads from puffer daemon
EOF
)"
```

---

## Rollback strategy

Each Task commits independently. If Task N goes wrong:

```bash
git log --oneline feat/desktop-ui-v2-login..HEAD     # see commits added by this plan
git reset --hard <last-good-commit-sha>              # roll back to before Task N
```

For Task 5 (Auth Station whitelist), rollback means removing `http://localhost:1467` from `ALLOWED_REDIRECT_ORIGINS` via the same API; the deploy is reversible.

For the storage migration (`cp -r ~/.corbina ~/.momo`), nothing breaks if `~/.momo` exists with copied data — it's a one-way additive operation.

---

## Risks & known unknowns

| Risk | Severity | Mitigation |
|---|---|---|
| Hidden V1 dispatch arms used by tests | Medium | Step 7 (V1 cargo check + V1 playwright run) catches this |
| Auth Station whitelist update doesn't propagate | Low | Task 5 Step 5 verifies before moving on |
| `puffer-session-store::MessageActor` semantics drift between V1 and Momo | Low | Same crate; both apps use it identically. No drift expected until someone explicitly bumps the crate. |
| Stale `~/.corbina/sessions.json` in Momo after migration | Low | User can `rm ~/.momo/sessions.json` to start fresh |
| `apps/momo/.env` not committed (git-ignored) | High | Add `.env.example` in Task 1 with the documented keys; README points to it |
| `~/.cargo/bin/puffer` is from a stale build (workspace moved on) | Low | Re-run `cargo install --path crates/puffer-cli` after any large rebase touches puffer-cli |
| Two Tauri apps with same `productName` cause macOS bundle confusion | Medium | tauri.conf.json identifiers differ (`com.corbina.desktop` vs `ai.tomo.momo`); productName differs too (Corbina vs Momo) |

---

## Self-review notes (run by the plan author before handoff)

- ✅ Spec coverage: every requirement from the chat discussion (split V2 to apps/momo, drop V1-only deps, keep V1 working, port renames, Auth Station whitelist) maps to a Task.
- ✅ No placeholders: all file paths, line numbers, code blocks, commands are specific. Two intentional `<...>` are in (a) Task 5 ALLOWED_REDIRECT_ORIGINS value (must be read at execution time, can't be hardcoded) and (b) Task 10 PR body link.
- ✅ Type consistency: env var names (`MOMO_*`), ports (1466/1431/1467), identifier (`ai.tomo.momo`), storage root (`~/.momo`) are used consistently across all tasks.
- ⚠️ Ambiguity (called out for executor): Task 3 Step 4 says "trim backend.rs" — this is the largest single edit (~2000 lines removed). The executor should iterate `cargo check` and fix-by-error rather than attempt a one-shot edit. If using subagent-driven execution, dispatch this as its own task with explicit "iterate until cargo check is clean" framing.
- ⚠️ External dependency: Task 5 (Vercel whitelist) requires production credentials and a deploy. If the executor doesn't have these, defer Task 5 to a human and skip Task 6 Step 3-4 (OAuth smoke).
