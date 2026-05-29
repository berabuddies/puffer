# Momo

Standalone Tauri desktop app: puffer-powered chat + WorldClaw U-card wallet.

Forked out of `apps/puffer-desktop/src-v2/` and that codebase's V2 src-tauri pieces (May 2026).

## Provider

Puffer-only. The host expects `puffer` on `PATH`, or honors `MOMO_PUFFER_BIN` for an explicit path.

## Storage

Default app home is `~/.momo` (override with `MOMO_HOME`).

- `sessions.json` — chat session metadata + transcripts
- `config.json` — UI config
- `credentials.json` — API keys
- `permissions.json`, `pins.json` — UI state

If you're migrating from V1 (Corbina): `cp -r ~/.corbina ~/.momo` before first run. The Momo backend prints a stderr hint if it sees this misconfiguration on startup.

## Auth

OAuth flow: webview redirects to OS browser, browser hits `http://localhost:1457/callback`, Momo's loopback HTTP listener catches it. Port 1457 is shared with Corbina (V1) — only one of the two apps can have OAuth listening at a time.

Auth Station must also allow-list `http://localhost:1457` in its `ALLOWED_REDIRECT_ORIGINS`, or the browser redirect is rejected.

`apps/momo/.env` (copy from `.env.example`):

```
VITE_AUTH_STATION_URL=https://auth.worldrouter.ai
```

## Development

Commands below use **npm** (`npm install`) — that's what the README and Tauri's
`beforeDevCommand` (`npm run ...`) assume. A `pnpm-lock.yaml` is also committed; if you
prefer pnpm, stick to one and don't mix the two lockfiles.

```bash
# 1. Frontend env — REQUIRED for login. Without it the Sign-in button silently does nothing.
cp .env.example .env

# 2. Make the `puffer` agent binary resolvable, or chat fails on the first message
#    with "`puffer` is not installed or not executable". Pick ONE:
cargo install --path ../../crates/puffer-cli           # installs onto ~/.cargo/bin (PATH)
# export MOMO_PUFFER_BIN="$(git rev-parse --show-toplevel)/target/debug/puffer"  # or reuse the workspace debug build

# 3. Install deps and run.
npm install
npm run check
npm run tauri dev
```

> First `tauri dev` runs `beforeDevCommand`, which compiles the whole Rust workspace
> (puffer-cli + deps) before the window opens — this can take several minutes with no
> progress output. It is not hung.

Vite serves on port 1466. Tauri WS backend listens on 1431. OAuth loopback on 1457.
These ports must be free: Corbina / puffer-desktop share 1431 and 1457, and a bind
clash fails **silently** (logged to stderr only) — the app still opens but never connects.

## Verification

```bash
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri build
```
