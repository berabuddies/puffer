#!/usr/bin/env bash
# Guest provisioning for the WeChat instance — enables BOTH:
#   1. the AT-SPI operate path (accessibility bridge + bus), and
#   2. Apple-Virt remote access (sshd + virtiofs + KasmVNC reachable over NAT).
#
# Designed to run inside our controlled image/rootfs (Docker container today, the
# VZ Linux guest later) so the same setup serves both runtimes. It deliberately
# does NOT download WeChat — the rootfs build provides an AT-SPI-capable WeChat.
# WeChat 4.x Linux is a Qt app and exposes the AT-SPI tree once this a11y env is in
# place BEFORE launch (verified on 4.1.1.4 and the latest 4.1.1.7). Idempotent.
set -euo pipefail

DISPLAY="${DISPLAY:-:1}"
export DISPLAY

log() { echo "[guest-setup] $*"; }

# --- 1. Accessibility stack (the AT-SPI operate path) ------------------------
ensure_pkgs() {
  # The locator (a11y_locate.py) uses the gi.Atspi GObject-introspection binding,
  # NOT pyatspi — match the binding the baked image / apply-runtime-a11y.sh install.
  command -v at-spi-bus-launcher >/dev/null 2>&1 \
    && python3 -c 'import gi; gi.require_version("Atspi", "2.0"); from gi.repository import Atspi' >/dev/null 2>&1 \
    && return 0
  log "installing AT-SPI stack"
  for i in $(seq 1 60); do fuser /var/lib/dpkg/lock-frontend >/dev/null 2>&1 || break; sleep 2; done
  apt-get update -qq
  apt-get install -y -qq at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11 >/dev/null
}

# Accessibility env MUST be in WeChat's environment before it launches, so the
# atk-bridge loads. Persist it for every login shell + the WM autostart.
write_a11y_env() {
  cat >/etc/profile.d/10-wechat-a11y.sh <<'EOF'
export QT_ACCESSIBILITY=1
export QT_LINUX_ACCESSIBILITY_ALWAYS_ON=1
export GTK_MODULES="${GTK_MODULES:+$GTK_MODULES:}gail:atk-bridge"
export NO_AT_BRIDGE=0
EOF
}

# Bring up a session D-Bus (if none) + the AT-SPI bus, then mark a screen reader
# active so Chromium-family UIs also enable accessibility. Writes the bus address
# to /run/wechat/dbus-addr for the connector's exec env.
start_a11y_bus() {
  mkdir -p /run/wechat
  if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    eval "$(dbus-launch --sh-syntax)"
  fi
  echo -n "$DBUS_SESSION_BUS_ADDRESS" >/run/wechat/dbus-addr
  pkill -f at-spi-bus-launcher 2>/dev/null || true
  sleep 1
  setsid /usr/libexec/at-spi-bus-launcher --launch-immediately >/var/log/atspi.log 2>&1 </dev/null &
  sleep 2
  dbus-send --session --type=method_call --dest=org.a11y.Bus /org/a11y/bus \
    org.freedesktop.DBus.Properties.Set string:org.a11y.Status string:IsEnabled \
    variant:boolean:true 2>/dev/null || true
  dbus-send --session --type=method_call --dest=org.a11y.Bus /org/a11y/bus \
    org.freedesktop.DBus.Properties.Set string:org.a11y.Status string:ScreenReaderEnabled \
    variant:boolean:true 2>/dev/null || true
  log "a11y bus: $(dbus-send --session --print-reply --dest=org.a11y.Bus /org/a11y/bus org.a11y.Bus.GetAddress 2>&1 | grep -c unix) up"
}

# --- 2. Remote access for the Apple-Virt backend -----------------------------
# In a VM there is no `docker exec`; the connector reaches the guest over SSH and
# exchanges files over a virtiofs share. (No-op extras in the Docker runtime.)
ensure_sshd() {
  [ "${WECHAT_VZ_GUEST:-0}" = "1" ] || return 0
  command -v sshd >/dev/null 2>&1 || { apt-get install -y -qq openssh-server >/dev/null; }
  install -d -m 0700 /home/abc/.ssh
  # The host injects its public key via the virtiofs share at boot.
  if [ -f /mnt/wxshare/authorized_keys ]; then
    install -m 0600 /mnt/wxshare/authorized_keys /home/abc/.ssh/authorized_keys
    chown -R abc:abc /home/abc/.ssh
  fi
  sed -i 's/^#\?PasswordAuthentication.*/PasswordAuthentication no/' /etc/ssh/sshd_config
  ssh-keygen -A
  service ssh restart 2>/dev/null || /usr/sbin/sshd
}

mount_share() {
  [ "${WECHAT_VZ_GUEST:-0}" = "1" ] || return 0
  install -d /mnt/wxshare
  mountpoint -q /mnt/wxshare || mount -t virtiofs wxshare /mnt/wxshare 2>/dev/null || true
}

# KasmVNC must be reachable from the host over the NAT IP (not just loopback) in
# the VM; in Docker we keep the published-port model and this is a no-op.
bind_kasm_for_vm() {
  [ "${WECHAT_VZ_GUEST:-0}" = "1" ] || return 0
  # KasmVNC config: listen on all interfaces inside the guest; the host reaches
  # https://<guest-nat-ip>:<port>. Loopback-only is preserved on the HOST side by
  # the NAT (the guest IP is on bridge100, not routable off-box).
  export KASM_BIND="0.0.0.0"
}

main() {
  ensure_pkgs
  write_a11y_env
  ensure_sshd
  mount_share
  bind_kasm_for_vm
  start_a11y_bus
  log "guest ready (a11y + remote access)"
}

main "$@"
