---
name: cua-dev
description: Guide for developing Puffer's computer-use integration and vendored CUA driver. Use when Codex works on vendor/cua-driver, scripts/build-cua-driver.sh, scripts/cua-computer-sandbox.sh, docker/cua-driver-native, CUA MCP manifests, screenshots/image tool-result handling, or specs/puffer-core/118.md.
---

# CUA Dev

Use this skill for Puffer computer-use work. Puffer's model loop is the agent;
CUA provides desktop tools over MCP or the vendored `cua-driver` binary.

## Read The Right Source First

Choose the reference before editing:

- `specs/puffer-core/118.md`: integration topology, image tool-result contract,
  native container path, desktop path, and known quirks.
- `vendor/cua-driver/PUFFER_NOTICE.md`: vendoring boundary and license rules.
- `vendor/cua-driver/Skills/cua-driver/SKILL.md`: runtime tool behavior and
  operating invariants for driving GUI apps.
- `vendor/cua-driver/Skills/cua-driver/MACOS.md`,
  `WINDOWS.md`, or `LINUX.md`: platform-specific behavior.
- `vendor/cua-driver/PARITY.md`: parity expectations against upstream CUA
  surfaces and known divergences.

## Choose The Integration Path

Match the task to one of the existing paths:

- **Sandbox bridge**: `scripts/cua-computer-sandbox.sh` runs CUA's Python
  `computer-server` in an XFCE Docker sandbox and exposes streamable HTTP MCP
  at `http://localhost:8000/mcp/`.
- **Native container**: `docker/cua-driver-native/` builds a Linux container
  with the Rust `cua-driver mcp` server. Use stdio via `docker exec -i`.
  Pass `--no-overlay` under Xvfb unless a compositor is known to alpha-blend the
  overlay correctly.
- **Desktop vendored driver**: `vendor/cua-driver/` builds separately from the
  Puffer workspace. `scripts/build-cua-driver.sh` installs it to
  `$PUFFER_HOME/bin/cua-driver` and writes a user-scoped MCP manifest. Do not
  commit generated manifests under `$PUFFER_HOME`.

Do not replace Puffer's model loop with CUA's own agent loop unless the user
explicitly asks for an experiment and the licensing/runtime implications are
spelled out.

## Editing Rules

- Treat `vendor/cua-driver` as vendored third-party MIT source. Preserve
  `LICENSE` and `PUFFER_NOTICE.md`.
- Do not vendor CUA `som`, `cua-agent[omni]`, or other AGPL-3.0 components.
- Keep Puffer-facing manifests, scripts, and specs outside the vendored
  workspace unless the change belongs to the driver itself.
- The root `Cargo.toml` excludes `vendor/cua-driver`; run vendor cargo commands
  from `vendor/cua-driver`.
- Do not wire telemetry, analytics, or feedback reporting into Puffer. If
  touching upstream telemetry-shaped code inside the vendor tree, keep it
  isolated from Puffer defaults.
- Preserve Puffer repo constraints for new Rust code: docstrings for public
  functions, ASCII unless existing context requires otherwise, small modules,
  and no new Rust source file over 1000 lines.

## Load-Bearing Contracts

Computer-use only works when screenshots reach the model as image content.
When changing MCP, runner, or tool-result code, protect this contract:

- Screenshot-like MCP results must preserve image parts, not flatten them into
  text-only summaries.
- Puffer should surface image tool results to the provider path that supports
  image inputs.
- A multi-tool-call turn should be able to loop through screenshot, reason,
  click/type, and screenshot again.

For Linux native containers, remember the current quirks documented in
`specs/puffer-core/118.md`: overlay can make screenshots black under Xvfb, and
`type_text` has synthetic input limitations. Prefer fixing the backend
deterministically over adding model prompt workarounds.

For macOS desktop work, preserve CUA's background-driving principle. Avoid
changes that steal focus, move the real cursor unnecessarily, or require
manual app foregrounding except where platform permissions demand it.

## Validation

Pick the narrowest validation that proves the change:

```bash
cargo fmt --check
cargo test --workspace -- --test-threads=1
```

Use the root workspace tests for Puffer-facing changes. For vendored driver
changes, run from the vendor workspace:

```bash
cd vendor/cua-driver
cargo fmt --check
cargo test -p cua-driver
```

For sandbox bridge changes:

```bash
./scripts/cua-computer-sandbox.sh status
./scripts/cua-computer-sandbox.sh shot /tmp/cua-shot.png
```

For desktop driver install or manifest changes:

```bash
./scripts/build-cua-driver.sh
puffer mcp list
```

For platform-specific CUA behavior, run or update the relevant integration or
parity tests under `vendor/cua-driver/tests/integration/` when the host OS can
support them. If a test is OS-specific and cannot run locally, state that
clearly and include the exact command that should be run on that platform.

## Spec Updates

When changing CUA behavior or integration contracts, add the next numbered spec
under the component you touched. Use `specs/puffer-core/` for model-loop, MCP,
script, or manifest integration changes, and keep `specs/puffer-core/118.md` as
the overview only when the overview itself needs correction.
