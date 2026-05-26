# Native computer-use sandbox: CUA's Rust `cua-driver` + a headless Xvfb desktop.
# No Python — this is the Phase 2 (native) counterpart to the Phase 1 MCP bridge
# (see specs/puffer-core/118.md).
#
# Build context MUST be the cua-driver rust workspace, e.g. a checkout of
# trycua/cua at libs/cua-driver/rust:
#
#   git clone --depth 1 https://github.com/trycua/cua /tmp/cua
#   cp docker/cua-driver-native/{Dockerfile,start-desktop.sh} /tmp/cua/libs/cua-driver/rust/
#   docker build --platform linux/arm64 -t cua-driver-native:latest \
#     -f /tmp/cua/libs/cua-driver/rust/Dockerfile /tmp/cua/libs/cua-driver/rust
#
# linux/arm64 builds natively on Apple Silicon (OrbStack); cua-driver's Linux
# backend is pure-Rust (x11rb/image/tiny-skia), no C toolchain needed.

# ---- stage 1: build cua-driver (Linux) ----
FROM rust:bookworm AS builder
WORKDIR /src
COPY . /src
RUN cargo build --release -p cua-driver && cp target/release/cua-driver /cua-driver

# ---- stage 2: runtime desktop ----
FROM debian:bookworm-slim
ENV DEBIAN_FRONTEND=noninteractive DISPLAY=:99
RUN apt-get update && apt-get install -y --no-install-recommends \
      xvfb x11-utils xauth openbox xterm \
      dbus-x11 at-spi2-core \
      fonts-dejavu-core ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=builder /cua-driver /usr/local/bin/cua-driver
COPY start-desktop.sh /usr/local/bin/start-desktop.sh
RUN chmod +x /usr/local/bin/start-desktop.sh
# The container hosts the desktop; `cua-driver mcp` is driven per-connection via
# `docker exec -i` (see the puffer MCP manifest in specs/puffer-core/118.md).
CMD ["/usr/local/bin/start-desktop.sh"]
