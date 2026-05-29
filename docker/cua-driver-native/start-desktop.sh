#!/bin/bash
# Boot a headless X desktop for cua-driver to act on, then idle.
# `cua-driver mcp` is launched per-connection via `docker exec -i`.
set -e
rm -f /tmp/.X99-lock
Xvfb :99 -screen 0 1280x800x24 -ac +extension RANDR +extension XTEST +extension COMPOSITE &
for _ in $(seq 1 40); do xdpyinfo -display :99 >/dev/null 2>&1 && break; sleep 0.5; done
# Session bus for AT-SPI accessibility tools; export so exec'd shells can source it.
eval "$(dbus-launch --sh-syntax)"
echo "export DBUS_SESSION_BUS_ADDRESS='${DBUS_SESSION_BUS_ADDRESS}'" > /tmp/dbus-env
openbox &
# allowSendEvents:true — cua-driver's Linux type_text targets a window via
# XSendEvent; xterm ignores synthetic events unless this resource is set.
xterm -xrm 'XTerm*allowSendEvents:true' -geometry 100x30+40+40 &
echo "desktop ready on :99"
exec sleep infinity
