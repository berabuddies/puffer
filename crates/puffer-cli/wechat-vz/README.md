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

WeChat 4.x Linux is a **Qt app** whose accessibility bridge is OFF by default; it
exposes an AT-SPI tree only when `QT_ACCESSIBILITY=1` /
`QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1` and the at-spi2 D-Bus bridge are set up
**before** the client launches, with `/config/.cache` owned by the app user. The
bare WechatOnCloud base does none of this, so this image (`image/`) bakes that
environment in.

> **Correction (2026-06-12).** An earlier note here claimed the latest "Universal"
> 4.x build had *dropped* AT-SPI (self-contained Radium/Chromium, no GTK/Qt). That
> was **WRONG** — empirically disproven: the current latest **4.1.1.7** exposes a
> full, live AT-SPI tree (frame + 21 buttons + 36 labels), and a 0-vision send was
> verified end-to-end through the connector. "Universal" is a Tencent CDN *path*,
> not a version; the binary is Qt with `AtSpiAdaptor` compiled in (the
> `RadiumWMPF` Chromium bits are only the mini-program sandbox). The old "0 apps"
> reading was an env false-negative (bridge not in WeChat's own process /
> root-owned `.cache`). **4.1.1.4 and 4.1.1.7 both work; the image defaults to
> 4.1.1.7** and is not capability-pinned.

## Components in this dir

- `a11y_locate.py` — AT-SPI locator: find an element by role/name → pixel
  bounds/center, read states. Runs in the guest/container; embedded into the
  connector via `include_str!` and pushed in at runtime.
- `image/` — `Dockerfile` + `build-image.sh` build `puffer-wechat-atspi:<version>`
  (currently **4.1.1.7**): `FROM ghcr.io/gloridust/wechat-on-cloud` + the a11y
  stack (at-spi2-core, gir1.2-atspi-2.0, python3-gi, dbus-x11). `build-image.sh`
  fetches the latest Universal WeChat `.deb` from Tencent and tags the image with
  its actual version (override with `WECHAT_DEB_URL` / `WECHAT_ATSPI_IMAGE`).
  `99-seed-wechat` (root cont-init) seeds the baked WeChat into `/config`, hands
  `/config/.cache` to the app user (root-owned .cache otherwise blocks the a11y bus
  socket), and makes `/run/wechat` writable. `autostart` starts a session D-Bus +
  at-spi-bus + the a11y env before WeChat and publishes the bus address to
  `/run/wechat/dbus-addr`.
- `guest-setup.sh` — equivalent provisioning for a VM guest (a11y stack + sshd +
  virtiofs + KasmVNC over NAT), for the container/VM runtime below.

The connector's `DEFAULT_IMAGE` is `puffer-wechat-atspi:4.1.1.7`; override per
instance with `WECHAT_IMAGE=puffer-wechat-atspi:<version>`.

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

**Exposure note.** KasmVNC is gated by basic-auth (a random per-instance password,
never shown by default). Docker publishes the desktop only on loopback; on
`container` it is reached at the container's vmnet IP, which is host-routable and
reachable by co-resident VMs on the same vmnet — a slightly wider surface than
loopback, still behind the basic-auth gate. Apple `container` 1.x has no
host-only network mode, so loopback parity is not yet available.

**Vision is off by default.** The connector drives WeChat via the AT-SPI
accessibility tree; the vision model (screen reading) costs tokens and is NOT an
automatic fallback — set `WECHAT_ALLOW_VISION=1` to permit it for a run.

Validated live on macOS 26.3.1: `auto` selects `container`, the WeChat desktop
comes up (a11y bus + client) on both 4.1.1.4 and the latest 4.1.1.7, a real-account
QR login + a 0-vision message send succeed, `exec` (stdout/stdin/env/`--user`) and
`--mount` persistence work, and the instance survives a stop/start (stable uid).
