# Connector image: WechatOnCloud + pinned AT-SPI-capable WeChat 4.1.1.4 + the
# accessibility stack, so the connector can drive the client through the AT-SPI
# tree. The latest Universal WeChat dropped AT-SPI; 4.1.1.4 exposes the full tree.
FROM ghcr.io/gloridust/wechat-on-cloud:latest

# AT-SPI accessibility stack (bus launcher + the gi.Atspi binding the client
# registers against) and dbus for the session bus.
RUN apt-get update && apt-get install -y --no-install-recommends \
        at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11 \
    && rm -rf /var/lib/apt/lists/*

# Pinned WeChat 4.1.1.4 (fetched into the build context by build-image.sh from
# agent-wechat's prebuilt image); seeded into /config on first run.
COPY wc411/ /opt/wechat-pinned/

# a11y-enabled autostart (brings up dbus + at-spi + a11y env before WeChat) and
# the root seed/permission hook.
COPY autostart /defaults/autostart
COPY 99-seed-wechat /custom-cont-init.d/99-seed-wechat
RUN chmod +x /defaults/autostart /custom-cont-init.d/99-seed-wechat
