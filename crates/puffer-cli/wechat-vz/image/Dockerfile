# Connector image: WechatOnCloud + an AT-SPI-capable WeChat + the accessibility
# stack, so the connector can drive the client through the AT-SPI tree. WeChat 4.x
# Linux is a Qt app and exposes the AT-SPI tree once the a11y env is set up before
# launch (the bare base does not). Verified on 4.1.1.4 and the latest 4.1.1.7.
FROM ghcr.io/gloridust/wechat-on-cloud:latest

# Everything the connector needs at RUNTIME, baked in so the running container
# never has to apt-get (its DNS/network can be restricted even when the host's
# isn't — and a failed runtime apt would silently break the monitor, DB read, and
# delivery checks):
#   - AT-SPI stack (bus launcher + the gi.Atspi binding the client registers
#     against) + dbus for the session bus — the no-vision operate path;
#   - imagemagick (`import`) — the screenshot tool the (optional) vision path and
#     the act delivery/recipient checks use;
#   - python3-pycryptodome + python3-zstandard — the direct chat-DB reader's
#     crypto/decompress deps (the monitor + read_history path).
# ForceIPv4 + retries + a timeout make the apt step survive a flaky build network
# (Apple `container`'s build VM in particular has spotty mirror reachability;
# harmless under Docker).
RUN apt-get -o Acquire::ForceIPv4=true -o Acquire::Retries=8 -o Acquire::http::Timeout=30 update \
    && apt-get -o Acquire::ForceIPv4=true install -y --no-install-recommends \
        at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11 \
        imagemagick python3-pycryptodome python3-zstandard \
    && rm -rf /var/lib/apt/lists/*

# The WechatOnCloud base bundles a Docker-in-Docker s6 service (`svc-docker`).
# Unprivileged, dockerd can't start (modprobe/mount fail), so s6 respawns it every
# few seconds forever — pure CPU + log noise; WeChat needs no inner Docker. Drop it
# from the s6 boot bundle (the service definition stays; it just never starts).
RUN rm -f /etc/s6-overlay/s6-rc.d/user/contents.d/svc-docker

# WeChat (fetched into the build context by build-image.sh — the latest Universal
# .deb, currently 4.1.1.7); seeded into /config on first run.
COPY wc411/ /opt/wechat-pinned/

# a11y-enabled autostart (brings up dbus + at-spi + a11y env before WeChat) and
# the root seed/permission hook.
COPY autostart /defaults/autostart
COPY 99-seed-wechat /custom-cont-init.d/99-seed-wechat
RUN chmod +x /defaults/autostart /custom-cont-init.d/99-seed-wechat
