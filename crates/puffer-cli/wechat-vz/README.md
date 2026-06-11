# WeChat connector: accessibility-tree operation + container image

Two related pieces:

1. **Accessibility-tree operation.** The connector drives WeChat through the OS
   AT-SPI accessibility tree (element role/name/bounds) instead of reading the
   screen with the vision model. This reduces token usage for the operate path
   (open chat / verify / send / quote); the vision model stays as the automatic
   fallback when the tree isn't reachable. The pat action still reads the screen
   (the avatar isn't exposed as an accessibility element).
2. **A pinned image** that makes (1) work, and that a non-Docker runtime can also
   run.

## Why a custom image

The current WeChat 4.x "Universal" build is a self-contained Radium/Chromium app
that links no system GTK/Qt, so the atk-bridge can't attach → **no AT-SPI**
(verified: 0 accessible apps even with the full env). **WeChat 4.1.1.4 does expose
the full tree.** So the image (`image/`) pins 4.1.1.4 and bakes the accessibility
environment in.

## Components in this dir

- `a11y_locate.py` — AT-SPI locator: find an element by role/name → pixel
  bounds/center, read states. Runs in the guest/container; embedded into the
  connector via `include_str!` and pushed in at runtime.
- `image/` — `Dockerfile` + `build-image.sh` build `puffer-wechat-atspi:4.1.1.4`:
  `FROM ghcr.io/gloridust/wechat-on-cloud` + the a11y stack (at-spi2-core,
  gir1.2-atspi-2.0, python3-gi, dbus-x11). `build-image.sh` extracts WeChat
  4.1.1.4 from `ghcr.io/thisnick/agent-wechat`. `99-seed-wechat` (root cont-init)
  seeds 4.1.1.4 into `/config`, hands `/config/.cache` to the app user (root-owned
  .cache otherwise blocks the a11y bus socket), and makes `/run/wechat` writable.
  `autostart` starts a session D-Bus + at-spi-bus + the a11y env before WeChat and
  publishes the bus address to `/run/wechat/dbus-addr`.
- `guest-setup.sh` — equivalent provisioning for a VM guest (a11y stack + sshd +
  virtiofs + KasmVNC over NAT), for the container/VM runtime below.

Point the connector at the image with `WECHAT_IMAGE=puffer-wechat-atspi:4.1.1.4`.

## Operate path, per action (current)

1. `open_chat`: click the recipient's row in the LEFT conversation list via the
   accessibility tree (no search box, so no web-result row can hijack the window);
   falls back to search + screen-reading if the chat isn't in the visible list.
2. verify open chat: chat-header label name == recipient (fail-closed; screen
   read as fallback for decorative names).
3. send: click the message-input bounds → xdotool type → Enter.
4. confirm sent: the sent body appears as a history bubble (screen read fallback).

## Runtime backend (Docker or Apple `container`)

`WechatInstance` (`wechat_connector/docker.rs`) drives either runtime through one
flag-compatible CLI surface (`run`/`exec`/`stop`/`rm`). Selection:
`WECHAT_RUNTIME=auto|docker|container` — `auto` prefers Apple `container` on
macOS 26+ when its CLI is installed (no Docker Desktop needed) and falls back to
Docker otherwise. `DOCKER_BIN` / `WECHAT_CONTAINER_BIN` override the binaries.

| concern | Docker (fallback) | Apple `container` (macOS 26+) |
|---|---|---|
| lifecycle | `docker run/start/stop/rm` | `container run/stop/rm` |
| exec | `docker exec [-i] --user -e` | same flags (validated) |
| volume | `-v vol:/config` | `--mount type=volume,source=vol,target=/config` |
| desktop reach | published `127.0.0.1:<port>` | **vmnet IP `<container-ip>:3000`** (no loopback port forwarding) |
| image source | local build / pull | `container image load` of a docker archive, or `pull` |

Puffer makes the `container` path turnkey (no terminal), via `ensure_container` →
`ensure_runtime_ready`:

- **apiserver** — starts it (`container system start`) and waits until it
  answers. Requires Homebrew `container` >= 1.0.0_1: the 1.0.0 bottle mislocated
  its plugins so the apiserver crash-looped "cannot find any plugins with type
  network" and hung every command (homebrew-core PR #286989 fixed it — if you
  hit it, `brew update && brew upgrade container`).
- **kernel** — installs the recommended VM kernel lazily on the first `run`
  (`container system kernel set --recommended` re-downloads each call, so it's
  not run eagerly).
- **image** — `pull` (registry ref) or `load` from `WECHAT_CONTAINER_IMAGE_TAR`.
- **desktop URL** — resolved to the container's vmnet IP, since `container` does
  not forward published ports to loopback.
- **uid stability** — the PUID/PGID remap is dropped on `container` (the
  VM-backed store needs no host-uid matching). The remap is non-deterministic
  across boots there; leaving `abc` at the image-default uid keeps the login
  data's ownership consistent, so WeChat survives a stop/start instead of dying
  on an unreadable, wrong-uid profile.

Validated live on macOS 26.3.1: `auto` selects `container`, the WeChat 4.1.1.4
desktop comes up (a11y bus + client), a real-account QR login + a 0-vision
message send succeed, `exec` (stdout/stdin/env/`--user`) and `--mount`
persistence work, and the instance survives a stop/start (stable uid).
