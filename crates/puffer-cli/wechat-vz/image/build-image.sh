#!/usr/bin/env bash
# Builds the connector image with pinned AT-SPI-capable WeChat 4.1.1.4.
# The 4.1.1.4 install is fetched at build time from agent-wechat's prebuilt image
# (so the ~670MB blob is never committed to the repo).
set -euo pipefail
cd "$(dirname "$0")"

AW="${WECHAT_PIN_SOURCE:-ghcr.io/thisnick/agent-wechat:latest}"
TAG="${WECHAT_ATSPI_IMAGE:-puffer-wechat-atspi:4.1.1.4}"
DOCKER="${DOCKER_BIN:-docker}"

echo "[build] fetching pinned WeChat from $AW"
"$DOCKER" image inspect "$AW" >/dev/null 2>&1 || "$DOCKER" pull "$AW"
cid=$("$DOCKER" create "$AW")
trap '"$DOCKER" rm -f "$cid" >/dev/null 2>&1 || true' EXIT
rm -rf wc411 && mkdir wc411
"$DOCKER" cp "$cid:/opt/wechat/." wc411/
[ -x wc411/wechat ] || { echo "ERROR: WeChat binary not found in $AW:/opt/wechat"; exit 1; }
echo "[build] pinned WeChat: $(du -sh wc411 | cut -f1)"

echo "[build] building $TAG"
"$DOCKER" build -t "$TAG" .
rm -rf wc411
echo
echo "[build] done -> $TAG"
echo "Point the connector at it:  WECHAT_IMAGE=$TAG"
