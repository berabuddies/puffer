#!/usr/bin/env bash
# Layers the AT-SPI accessibility stack onto a stock WechatOnCloud BASE container
# at RUNTIME. This is the Docker-free fallback the connector takes when the baked
# a11y image cannot be built (Apple `container`'s build VM can't reach the apt
# mirrors). The *runtime* container has network (vmnet), so apt works here even
# though the build VM couldn't — that's the whole point of doing this at runtime.
#
# Runs as ROOT (it apt-installs and writes image-layer files; it drops to abc for
# ownership). Idempotent: it installs only what's missing and rewrites the
# autostart only when it differs, then reports whether anything CHANGED so the
# caller knows whether a container restart is needed (a restart is how openbox
# picks up the new autostart and (re)launches WeChat with the a11y env).
#
# Output (stdout): an `A11Y_READY=<0|1>` line (1 = a11y stack functional) and an
# `A11Y_CHANGED=<0|1>` line (1 = the caller should restart the container).
#
# The a11y-enabled openbox autostart is read from $A11Y_AUTOSTART_SRC (the
# connector pushes it there) — it is the SAME script the baked image bakes in, so
# there is one source of truth. The boot hook (01-woc-autostart) copies
# /defaults/autostart to the live openbox autostart on every start, so installing
# it to /defaults makes it authoritative across restarts.
set -u

AUTOSTART_SRC="${A11Y_AUTOSTART_SRC:-/tmp/puffer-a11y-autostart}"
changed=0
log() { echo "[runtime-a11y] $*" >&2; }

# 1) Accessibility packages. The base image ships none (verified: no
#    at-spi-bus-launcher, no gi.Atspi). gir1.2-atspi-2.0 + python3-gi provide the
#    GObject-introspection binding a11y_locate.py uses; at-spi2-core provides the
#    bus launcher; dbus-x11 provides dbus-launch. Robust apt for flaky mirrors.
# The at-spi bus launcher's path varies by distro/version (Debian has shipped it
# under /usr/libexec and under /usr/lib/<triplet>/at-spi2-core), so probe rather
# than hardcode — `command -v` does NOT find it (it is not on PATH).
find_launcher() {
  local p
  for p in /usr/libexec/at-spi-bus-launcher /usr/lib/at-spi2-core/at-spi-bus-launcher /usr/lib/*/at-spi2-core/at-spi-bus-launcher; do
    [ -x "$p" ] && { echo "$p"; return 0; }
  done
  command -v at-spi-bus-launcher 2>/dev/null
}
# a11y is functional when the gi.Atspi binding imports (gir1.2-atspi-2.0 +
# python3-gi) AND the bus launcher (at-spi2-core) is present.
have_a11y() {
  python3 -c 'import gi; gi.require_version("Atspi", "2.0"); from gi.repository import Atspi' >/dev/null 2>&1 \
    && [ -n "$(find_launcher)" ]
}
# Some container DNS setups don't resolve external hosts even though raw egress
# works, which makes `apt-get update` hang or fail. Only acts when the mirror
# doesn't already resolve (so a working/managed resolver is left untouched); see
# the inline note for why it prepends + forces TCP rather than appends.
ensure_dns() {
  getent hosts deb.debian.org >/dev/null 2>&1 && return 0
  # Public DNS is broken. Two failure modes seen in the wild: (a) the container's
  # embedded resolver doesn't resolve external hosts, (b) outbound UDP:53 is
  # blocked while TCP:53 is allowed (some sandboxes/cloud networks). Address both:
  # PREPEND 8.8.8.8 as PRIMARY (a broken resolver returning NXDOMAIN is
  # authoritative, so glibc never consults a secondary — 8.8.8.8 must be first),
  # and add `use-vc` to force DNS-over-TCP. Written in place (resolv.conf is
  # usually a bind mount that can't be renamed over); existing entries are kept.
  # Only runs when public DNS is already broken, so it can't regress a working
  # resolver, and the change reverts on the next container restart.
  log "the container resolver can't resolve the mirror; setting 8.8.8.8 + DNS-over-TCP"
  local orig; orig="$(grep -vE '^nameserver 8\.8\.8\.8$|^options use-vc$' /etc/resolv.conf 2>/dev/null)"
  printf 'nameserver 8.8.8.8\n%s\noptions use-vc\n' "$orig" > /etc/resolv.conf 2>/dev/null || true
  getent hosts deb.debian.org >/dev/null 2>&1
}
if ! have_a11y; then
  log "installing AT-SPI stack (at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11)…"
  # Wait out any concurrent apt holding EITHER the dpkg lock OR the apt-lists lock
  # — the base image runs its own apt during early boot, and waiting only on the
  # dpkg lock let `apt-get update` fail on the lists lock and skip the install.
  for _ in $(seq 1 90); do
    fuser /var/lib/dpkg/lock-frontend /var/lib/apt/lists/lock >/dev/null 2>&1 || break
    sleep 2
  done
  ensure_dns
  # DPkg::Lock::Timeout makes apt itself wait for the dpkg lock instead of erroring.
  APT="apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=8 -o Acquire::http::Timeout=30 -o DPkg::Lock::Timeout=120"
  pkgs="at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11"
  if ! { $APT update && $APT install -y --no-install-recommends $pkgs; }; then
    # Retry once, ensuring DNS again (the resolver may have needed the fallback).
    log "apt failed; ensuring DNS and retrying once…"
    ensure_dns
    $APT update && $APT install -y --no-install-recommends $pkgs || true
  fi
  if have_a11y; then
    changed=1
  else
    log "WARNING: AT-SPI stack still not present after apt; the no-vision operate path will not work"
  fi
fi

# 2) a11y prerequisites the baked image's seed hook normally sets up: the bus
#    socket lives under /config/.cache and the autostart records the bus address
#    under /run/wechat — both MUST be abc-writable (root-owned .cache is exactly
#    what blocks the AT-SPI bus from binding).
mkdir -p /config/.cache && chown -R abc:abc /config/.cache 2>/dev/null || true
mkdir -p /run/wechat && chown abc:abc /run/wechat 2>/dev/null || true

# 3) Install the a11y-enabled openbox autostart (idempotent: only when it differs
#    from what is already installed). Writing /defaults/autostart makes the boot
#    hook copy it to the live openbox autostart on every start; we also refresh
#    the live copy directly so a restart in this same boot picks it up.
if [ -f "$AUTOSTART_SRC" ]; then
  if ! cmp -s "$AUTOSTART_SRC" /defaults/autostart 2>/dev/null; then
    install -m 0755 "$AUTOSTART_SRC" /defaults/autostart
    mkdir -p /config/.config/openbox
    install -m 0755 "$AUTOSTART_SRC" /config/.config/openbox/autostart
    chown -R abc:abc /config/.config 2>/dev/null || true
    log "installed the a11y-enabled openbox autostart"
    changed=1
  fi
else
  log "WARNING: a11y autostart source $AUTOSTART_SRC not found; cannot enable a11y at launch"
fi

# 4) Persist the abc-ownership prep across restarts via a cont-init hook (the base
#    image has no seed hook of its own). Mirrors the baked image's 99-seed-wechat
#    ownership prep so /config/.cache + /run/wechat stay abc-writable on each boot.
seed=/custom-cont-init.d/98-puffer-a11y-prep
if [ ! -f "$seed" ]; then
  mkdir -p /custom-cont-init.d
  cat > "$seed" <<'SEED'
#!/usr/bin/with-contenv bash
# Puffer runtime-a11y: keep the AT-SPI bus socket dir + dbus-addr dir abc-writable
# on every boot (root-owned .cache blocks the bus).
mkdir -p /config/.cache && chown -R abc:abc /config/.cache 2>/dev/null || true
mkdir -p /run/wechat && chown abc:abc /run/wechat 2>/dev/null || true
SEED
  chmod +x "$seed"
fi

if have_a11y; then echo "A11Y_READY=1"; else echo "A11Y_READY=0"; fi
echo "A11Y_CHANGED=$changed"
