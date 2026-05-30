# Momo

Standalone Tauri desktop app: puffer-powered chat + WorldClaw U-card wallet.

> **Branch:** this app currently lives only on `feat/momo-desktop`, not on `master`.
> After cloning, `git checkout feat/momo-desktop` — otherwise `apps/momo` won't exist.

Forked out of `apps/puffer-desktop/src-v2/` and that codebase's V2 src-tauri pieces (May 2026).

## Prerequisites

- **Rust** stable toolchain (`cargo`) — builds the `puffer` agent and the Tauri host.
- **Node.js** ≥ 20 LTS + npm (verified on Node 25 / npm 11).
- **Tauri 2 system deps**:
  - macOS: Xcode Command Line Tools (`xcode-select --install`).
  - Linux: `libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev`.
  - Authoritative list: https://v2.tauri.app/start/prerequisites/
- For the Playwright UI tests (Verification below): `npx playwright install chromium webkit`.

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

OAuth flow: webview redirects to OS browser → browser hits `http://localhost:1457/callback` → Momo's loopback HTTP listener catches it. Port 1457 is shared with Corbina (V1) — only one app can hold the OAuth listener at a time.

Auth Station must also allow-list `http://localhost:1457` in its `ALLOWED_REDIRECT_ORIGINS`, or the browser redirect is rejected.

**Login depends on TWO external services**, and chat won't work until both succeed:

1. `VITE_AUTH_STATION_URL` (`auth.worldrouter.ai`) — the OAuth handshake.
2. `VITE_WORLDROUTER_CONTROL_URL` (`control-api.worldrouter.ai`) — after login, the JWT is exchanged here, in two hops, for the `sk-worldrouter-...` key that gets written to `$MOMO_HOME/credentials.json` and drives puffer.

If login looks successful but chat fails ("signed in but can't chat"), the key mint likely failed: check the browser console for mint errors and `$MOMO_HOME/credentials.json` for the puffer key.

## Environment

`apps/momo/.env` (copy from `.env.example`). Only `VITE_AUTH_STATION_URL` has no default — it's the one you must set; the rest fall back to the values below.

| var | when needed | default | purpose |
|---|---|---|---|
| `VITE_AUTH_STATION_URL` | login (required) | — | Auth Station; missing → Sign-in button silently no-ops |
| `VITE_WORLDROUTER_CONTROL_URL` | login → chat | `https://control-api.worldrouter.ai` | JWT → worldrouter API key exchange (2 hops) |
| `VITE_PUFFER_WS_URL` | optional | `ws://127.0.0.1:1431/ws` | frontend ↔ Momo backend WS |
| `VITE_BACKEND_BASE_URL` | wallet only | `http://127.0.0.1:8080` | U-card wallet REST base |
| `VITE_USE_MOCK_WALLET` | optional | unset | wallet/KYC mock toggle |

## Development

Commands below use **npm** (`npm install`) — that's what the README and Tauri's
`beforeDevCommand` (`npm run ...`) assume. A `pnpm-lock.yaml` is also committed; if you
prefer pnpm, stick to one and don't mix the two lockfiles.

```bash
# 1. Frontend env — REQUIRED for login. Without it the Sign-in button silently does nothing.
cp .env.example .env

# 2. Make the `puffer` agent binary resolvable, or chat fails on the first message
#    with "`puffer` is not installed or not executable".
#    RECOMMENDED — build in-tree and point Momo at it (does NOT touch your global PATH):
cargo build -p puffer-cli
export MOMO_PUFFER_BIN="$(git rev-parse --show-toplevel)/target/debug/puffer"
#    Alternative: `cargo install --path ../../crates/puffer-cli` installs onto ~/.cargo/bin —
#    convenient, but OVERWRITES any existing global `puffer`.
#    Note: `tauri dev` compiles puffer to target/debug via beforeDevCommand, but does NOT
#    put it on PATH — you still need one of the two options above.

# 3. Install deps and run.
npm install
npm run check
npm run tauri dev
```

> **Day-to-day (start & restart):** once `.env` and `puffer` are set up, just run
> `./scripts/dev.sh` (from `apps/momo`, or `apps/momo/scripts/dev.sh` from anywhere).
> It stops any running instance, reaps orphaned puffer daemons, then launches
> `tauri dev` — idempotent, so the same command doubles as a restart. It only ever
> touches this repo's `target/debug` binaries, never a global `puffer`.

> First `tauri dev` runs `beforeDevCommand`, which compiles the whole Rust workspace
> (puffer-cli + deps) before the window opens — this can take several minutes with no
> progress output. It is not hung.

Vite serves on port 1466. Tauri WS backend listens on 1431. OAuth loopback on 1457.
These ports must be free: Corbina / puffer-desktop share 1431 and 1457, and a bind
clash fails **silently** (logged to stderr only) — the app still opens but never connects.

## Verification

```bash
npm run check
npx playwright install chromium webkit   # one-time, for the UI tests below
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri build
```

> `tauri.conf.json` `bundle.targets` is currently set to Linux targets (`deb`, `appimage`);
> on macOS, adjust targets before `tauri build` (dev/run is unaffected).
