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

If you're migrating from V1 (Corbina): `cp -r ~/.corbina ~/.momo` before first run. The Momo backend prints a stderr hint if it sees this misconfiguration on startup.

## Auth

OAuth flow: webview redirects to OS browser, browser hits `http://localhost:1457/callback`, Momo's loopback HTTP listener catches it. Port 1457 is shared with Corbina (V1) — only one of the two apps can have OAuth listening at a time.

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

Vite serves on port 1466. Tauri WS backend listens on 1431. OAuth loopback on 1457.

## Verification

```bash
npm run check
npm run test:desktop-ui
cargo check --manifest-path src-tauri/Cargo.toml
npm run tauri build
```
