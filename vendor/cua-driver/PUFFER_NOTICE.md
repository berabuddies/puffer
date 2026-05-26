# Vendored third-party: cua-driver (from trycua/cua)

This directory is a **vendored copy** of CUA's Rust `cua-driver` computer-use
engine, from <https://github.com/trycua/cua> (`libs/cua-driver/rust`). It powers
Puffer's desktop computer-use feature (see `specs/puffer-core/118.md`).

- **License**: MIT — see `LICENSE` (Copyright (c) 2025 Cua AI, Inc.). Retained
  verbatim per the MIT notice requirement.
- **What this is**: the `cua-driver` binary + its platform backends
  (macOS / Windows / Linux) and stdio MCP server. macOS uses background driving
  (acts on a target window without stealing cursor/focus).
- **Built separately**: this is its own cargo workspace (excluded from Puffer's
  workspace in the root `Cargo.toml`). Build it with `scripts/build-cua-driver.sh`.
- **Not vendored on purpose**: CUA's `som` / `cua-agent[omni]` visual-grounding
  components are **AGPL-3.0** and are deliberately NOT included here. Do not add
  them — they would impose copyleft obligations on this repository.

Upstream revision: trycua/cua `cua-driver` v0.2.18.
To update: re-copy `libs/cua-driver/rust` (excluding `target/`, `.git/`) and keep
this NOTICE + the MIT `LICENSE`.
