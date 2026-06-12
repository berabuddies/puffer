# Connector image: WechatOnCloud + an AT-SPI-capable WeChat + the accessibility
# stack, so the connector can drive the client through the AT-SPI tree. WeChat 4.x
# Linux is a Qt app and exposes the AT-SPI tree once the a11y env is set up before
# launch (the bare base does not). Verified on 4.1.1.4 and the latest 4.1.1.7.
FROM ghcr.io/gloridust/wechat-on-cloud:latest

# AT-SPI accessibility stack (bus launcher + the gi.Atspi binding the client
# registers against) and dbus for the session bus.
RUN apt-get update && apt-get install -y --no-install-recommends \
        at-spi2-core gir1.2-atspi-2.0 python3-gi dbus-x11 \
    && rm -rf /var/lib/apt/lists/*

# WeChat (fetched into the build context by build-image.sh — the latest Universal
# .deb, currently 4.1.1.7); seeded into /config on first run.
COPY wc411/ /opt/wechat-pinned/

# a11y-enabled autostart (brings up dbus + at-spi + a11y env before WeChat) and
# the root seed/permission hook.
COPY autostart /defaults/autostart
COPY 99-seed-wechat /custom-cont-init.d/99-seed-wechat
RUN chmod +x /defaults/autostart /custom-cont-init.d/99-seed-wechat
