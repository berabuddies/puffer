# puffer-desktop native CEF — dev gate + packaging handoff

> 2026-06-02 · branch `feat/cef-dev-bridge-gate` (this branch) · owner sean
> Handoff to the `feat/macos-packaging` branch, which will rebase this branch.

## TL;DR

In dev (`tauri dev`) the embedded native CEF view does not exist, but
`daemon_launcher` was unconditionally telling the daemon's agent browser to
attach to it (CDP port 9333). The agent attached to nothing, fell back to a
standalone `--headless=new` Chrome, and that collided with the browser the user
sees in the pane. This branch gates the bridge so it is only wired when the
native CEF view can actually exist. The macOS packaging work must satisfy three
prerequisites (below) for a packaged release to actually use CEF.

## What this branch changes

`apps/puffer-desktop/src-tauri/src/daemon_launcher.rs` — the
`PUFFER_CEF_REMOTE_DEBUGGING_PORT=9333` injection in `spawn_daemon` went from
unconditional to gated:

```rust
let cef_bridge_enabled = !cfg!(debug_assertions)
    && cfg!(all(target_os = "macos", puffer_desktop_cef_native));
if cef_bridge_enabled && std::env::var_os("PUFFER_CEF_REMOTE_DEBUGGING_PORT").is_none() {
    cmd.env("PUFFER_CEF_REMOTE_DEBUGGING_PORT", "9333");
}
```

Behavior:

- **dev (`tauri dev`, debug build)**: bridge OFF. The daemon no longer attaches
  to a missing CEF view, so it stops spawning the conflicting headless Chrome.
  The user and the agent converge on the single screencast browser.
- **release with native CEF compiled in**: bridge ON — unchanged from before.
- An explicit pre-set `PUFFER_CEF_REMOTE_DEBUGGING_PORT` still wins (the bridge
  can be forced on for debugging).

Nothing else needed changing:

- The frontend already falls back on its own. `BrowserPane.svelte` derives
  `nativeCefReady` from `nativeCefStatus.available`; in dev
  `browser_cef_native_status` returns `available: false`, so it already uses the
  `"screencast"` renderer and never shows a native pane.
- `build.rs` already compiles native CEF out in dev (no runtime discovered →
  `puffer_desktop_cef_native` cfg unset).

## ⚠️ Rebase note for `feat/macos-packaging`

The packaging branch will very likely also touch that
`PUFFER_CEF_REMOTE_DEBUGGING_PORT` injection. On conflict, **keep the
`cef_bridge_enabled` gate — do not revert to unconditional injection.** The
release path (`!debug_assertions && puffer_desktop_cef_native`) already enables
the bridge, so the gate is compatible with packaging.

## Three prerequisites for a packaged release to actually use CEF

### 1. `puffer_desktop_cef_native` must be ON at compile time

`build.rs` only emits `rustc-cfg=puffer_desktop_cef_native` when
`CefBuildPaths::discover()` finds the CEF runtime **at build time** (via
`PUFFER_CEF_PATH` / `PUFFER_CEF_ROOT` / `CEF_PATH`, the
`target/puffer-cef-runtime` symlink, or `~/chromium_tintin/src/out/...`).

Consequence: **bundling the framework into the `.app` is not enough.** If the
release is compiled without the runtime discoverable, the cfg stays OFF, the
gate above keeps `cef_bridge_enabled == false`, and the packaged app will not
use CEF even though the framework is present. **Both** must hold: discoverable
at compile time **and** bundled at runtime.

### 2. The rpath is currently an absolute dev path — this is the real packaging fix

`build.rs` emits `cargo:rustc-link-arg=-Wl,-rpath,{absolute runtime_root}`,
pointing at the build machine's disk. That path will not exist on a user's
machine. Packaging must:

- add an `@executable_path/../Frameworks`-style rpath, and
- copy `Chromium Embedded Framework.framework` + `cefsimple Helper.app` into the
  app bundle's `Frameworks/`, and
- include the helper dylibs that `ensure_helper_library_links` wires up
  (`libEGL.dylib`, `libGLESv2.dylib`, `libvk_swiftshader.dylib`,
  `vk_swiftshader_icd.json`) under the framework's `Libraries/`.

### 3. There is no macOS bundling config yet

`apps/puffer-desktop/src-tauri/tauri.conf.json` has
`bundle.targets = ["deb", "appimage"]` (Linux only) and no macOS `frameworks`
section. This has to be added from scratch.

## Useful facts

- Prebuilt CEF source: `puffer-cef-macos-arm64.tar.gz` from the
  `berabuddies/ct` release. **bobo's `scripts/fetch-cef.sh` is a ready
  reference** (`gh release download` → symlink `target/bobo-cef-runtime`); it
  can be ported to a puffer-desktop `fetch-cef` to provision the runtime for
  both compile-time discovery and bundling.
- Self-check command: `browser_cef_native_status` returns `build_enabled` (cfg
  on?) and `available` (runtime discovered + initialized?). After packaging,
  run the app and confirm **both are true** — that is the signal CEF was both
  compiled in and is usable in the bundle.
