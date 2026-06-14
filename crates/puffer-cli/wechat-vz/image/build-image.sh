#!/usr/bin/env bash
# Builds the connector image: an AT-SPI-capable WeChat + the accessibility stack,
# so the connector can drive the client through the AT-SPI tree (no vision).
#
# WeChat 4.x Linux is a Qt app that exposes an AT-SPI tree once the a11y env is set
# up before launch (this image bakes that in). By DEFAULT this fetches the LATEST
# Universal WeChat .deb from Tencent and tags the image with its actual version
# (verified working on 4.1.1.4 and the latest 4.1.1.7 — there is no version that
# "lacks AT-SPI"). The ~670MB WeChat blob is never committed to the repo.
#
#   ./build-image.sh                            # build into the Docker image store
#   WECHAT_RUNTIME=container ./build-image.sh   # build into Apple container's store
#   WECHAT_DEB_URL=<url> ./build-image.sh        # build a specific WeChat .deb
#   WECHAT_ATSPI_IMAGE=name:tag ./build-image.sh # override the image tag
#   WECHAT_BUILD_DNS=8.8.8.8 ./build-image.sh    # DNS for `container build`'s VM
#   WECHAT_BUILD_RETRIES=2 ./build-image.sh      # `container build` retry count
#
# The .deb fetch + extract is Docker-free (curl + bsdtar). The image itself is
# built with the SELECTED runtime's native builder: `docker build` for docker,
# `container build` (in its own builder VM) for Apple `container` — so neither
# runtime needs the other installed. `container build`'s builder VM has spottier
# apt-mirror reachability than Docker, so its build is retried (and the Dockerfile
# hardens apt with ForceIPv4 + retries); if it still fails the connector falls
# back to the base image + runtime accessibility setup. Works on macOS (no dpkg)
# and Linux.
set -euo pipefail
cd "$(dirname "$0")"

case "$(uname -m)" in arm64|aarch64) DEB_ARCH=arm64;; *) DEB_ARCH=x86_64;; esac
DEB_URL="${WECHAT_DEB_URL:-https://dldir1v6.qq.com/weixin/Universal/Linux/WeChatLinux_${DEB_ARCH}.deb}"
DOCKER="${DOCKER_BIN:-docker}"                 # builds the image
CONTAINER="${WECHAT_CONTAINER_BIN:-container}" # Apple container, for the load step
RUNTIME="${WECHAT_RUNTIME:-docker}"            # docker | container (where to land it)

deb=""; work=""
trap 'rm -rf wc411 "${deb:-}" "${work:-}" 2>/dev/null || true' EXIT
rm -rf wc411 && mkdir wc411

echo "[build] fetching WeChat ($DEB_ARCH) from $DEB_URL"
deb="$(mktemp -t wechat-deb)"
curl -fSL -o "$deb" "$DEB_URL"

# Extract /opt/wechat + read the Version, portably: dpkg-deb on Linux, otherwise
# bsdtar (it reads the .deb `ar` archive) on macOS.
work="$(mktemp -d)"
if command -v dpkg-deb >/dev/null 2>&1; then
    VER="$(dpkg-deb -f "$deb" Version 2>/dev/null || true)"
    dpkg-deb -x "$deb" "$work"
else
    ( cd "$work" && tar -xf "$deb" )                       # -> control.tar.* + data.tar.*
    tar -C "$work" -xf "$work"/control.tar.* 2>/dev/null || true
    VER="$(awk -F': ' '/^Version:/{print $2; exit}' "$work/control" 2>/dev/null || true)"
    tar -C "$work" -xf "$work"/data.tar.*
fi
cp -a "$work/opt/wechat/." wc411/
[ -x wc411/wechat ] || { echo "ERROR: WeChat binary not found in $DEB_URL (/opt/wechat)"; exit 1; }

TAG="${WECHAT_ATSPI_IMAGE:-puffer-wechat-atspi:${VER:-latest}}"
echo "[build] WeChat ${VER:-unknown} -> $(du -sh wc411 | cut -f1); tag $TAG"

if [ "$RUNTIME" = "container" ]; then
    # Apple `container` keeps its own OCI image store (separate from Docker's) and
    # builds with `container build` (a buildkit instance in its own VM) — so this
    # path needs no Docker. The builder VM's apt-mirror reachability is spottier
    # than Docker's, so point it at a public resolver (--dns) and retry a few
    # times; the Dockerfile additionally hardens apt (ForceIPv4 + retries). If it
    # still fails, build-image.sh exits non-zero and the connector falls back to
    # the base image + runtime accessibility setup (no Docker either way).
    echo "[build] building $TAG with Apple container (container build --dns)"
    "$CONTAINER" system start >/dev/null 2>&1 || true
    dns="${WECHAT_BUILD_DNS:-8.8.8.8}"
    retries="${WECHAT_BUILD_RETRIES:-2}"
    attempt=1
    until "$CONTAINER" build --dns "$dns" -t "$TAG" .; do
        if [ "$attempt" -ge "$retries" ]; then
            echo "[build] container build failed after $attempt attempt(s)." >&2
            echo "[build] (the builder VM likely could not reach the apt mirrors; the" >&2
            echo "[build]  connector falls back to the base image + runtime a11y setup.)" >&2
            exit 1
        fi
        attempt=$((attempt + 1))
        echo "[build] container build failed; retrying ($attempt/$retries)…" >&2
        sleep 3
    done
else
    echo "[build] building $TAG with docker"
    "$DOCKER" build -t "$TAG" .
fi

echo
echo "[build] done -> $TAG (in the $RUNTIME image store)"
echo "Set this as the connector default by matching DEFAULT_IMAGE, or run against it:"
echo "  WECHAT_RUNTIME=$RUNTIME WECHAT_IMAGE=$TAG"
