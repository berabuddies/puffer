#!/usr/bin/env bash
# Builds the connector image with pinned AT-SPI-capable WeChat 4.1.1.4.
# The 4.1.1.4 install is fetched at build time from agent-wechat's prebuilt image
# (so the ~670MB blob is never committed to the repo).
#
#   ./build-image.sh                       # build into the Docker image store
#   WECHAT_RUNTIME=container ./build-image.sh   # build into Apple `container`'s store
#
# The WeChat extraction always uses Docker (it needs `create`/`cp`); only the
# final `build` targets the selected runtime's image store.
set -euo pipefail
cd "$(dirname "$0")"

AW="${WECHAT_PIN_SOURCE:-ghcr.io/thisnick/agent-wechat:latest}"
TAG="${WECHAT_ATSPI_IMAGE:-puffer-wechat-atspi:4.1.1.4}"
DOCKER="${DOCKER_BIN:-docker}"               # used to extract the pinned WeChat
RUNTIME="${WECHAT_RUNTIME:-docker}"          # docker | container (final build target)
case "$RUNTIME" in
  container) BUILDER="${WECHAT_CONTAINER_BIN:-container}" ;;
  *)         BUILDER="$DOCKER" ;;
esac

echo "[build] fetching pinned WeChat from $AW"
"$DOCKER" image inspect "$AW" >/dev/null 2>&1 || "$DOCKER" pull "$AW"
cid=$("$DOCKER" create "$AW")
trap '"$DOCKER" rm -f "$cid" >/dev/null 2>&1 || true' EXIT
rm -rf wc411 && mkdir wc411
"$DOCKER" cp "$cid:/opt/wechat/." wc411/
[ -x wc411/wechat ] || { echo "ERROR: WeChat binary not found in $AW:/opt/wechat"; exit 1; }
echo "[build] pinned WeChat: $(du -sh wc411 | cut -f1)"

echo "[build] building $TAG with $BUILDER (runtime=$RUNTIME)"
if [ "$RUNTIME" = "container" ]; then
    # `container build` runs an inner builder VM; size it for a multi-GB base
    # image and show plain progress (the default is silent until done).
    "$BUILDER" builder start --cpus "${BUILDER_CPUS:-4}" --memory "${BUILDER_MEM:-8g}" >/dev/null 2>&1 || true
    "$BUILDER" build --progress plain -t "$TAG" .
else
    "$BUILDER" build -t "$TAG" .
fi
rm -rf wc411
echo
echo "[build] done -> $TAG (in the $RUNTIME image store)"
echo "Run the connector against it:  WECHAT_RUNTIME=$RUNTIME WECHAT_IMAGE=$TAG"
