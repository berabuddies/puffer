#!/usr/bin/env bash
# Builds the connector image with pinned AT-SPI-capable WeChat 4.1.1.4.
# The 4.1.1.4 install is fetched at build time from agent-wechat's prebuilt image
# (so the ~670MB blob is never committed to the repo).
#
#   ./build-image.sh                            # build into the Docker image store
#   WECHAT_RUNTIME=container ./build-image.sh   # also load it into Apple container's store
#
# The image is always built with Docker; for the container runtime it is then
# loaded into container's separate OCI store via `docker save | container image
# load` (no inner builder VM needed — `container image load` takes the archive).
set -euo pipefail
cd "$(dirname "$0")"

AW="${WECHAT_PIN_SOURCE:-ghcr.io/thisnick/agent-wechat:latest}"
TAG="${WECHAT_ATSPI_IMAGE:-puffer-wechat-atspi:4.1.1.4}"
DOCKER="${DOCKER_BIN:-docker}"                 # builds the image + extracts WeChat
CONTAINER="${WECHAT_CONTAINER_BIN:-container}" # Apple container, for the load step
RUNTIME="${WECHAT_RUNTIME:-docker}"            # docker | container (where to land it)

echo "[build] fetching pinned WeChat from $AW"
"$DOCKER" image inspect "$AW" >/dev/null 2>&1 || "$DOCKER" pull "$AW"
cid=$("$DOCKER" create "$AW")
tmp=""  # save-tarball path (set later for the container load); cleaned on EXIT
# Clean up on any exit (incl. a failed build/save/load): the extraction
# container, the ~670MB wc411/ build context, and the temp save tarball.
trap '"$DOCKER" rm -f "$cid" >/dev/null 2>&1 || true; rm -rf wc411 "${tmp:-}" 2>/dev/null || true' EXIT
rm -rf wc411 && mkdir wc411
"$DOCKER" cp "$cid:/opt/wechat/." wc411/
[ -x wc411/wechat ] || { echo "ERROR: WeChat binary not found in $AW:/opt/wechat"; exit 1; }
echo "[build] pinned WeChat: $(du -sh wc411 | cut -f1)"

echo "[build] building $TAG with docker"
"$DOCKER" build -t "$TAG" .
rm -rf wc411

if [ "$RUNTIME" = "container" ]; then
    # Apple `container` keeps its own OCI image store (separate from Docker's).
    # Build with Docker above, then load the result in — `container image load`
    # accepts the docker archive, so no inner builder VM is involved.
    echo "[build] loading $TAG into Apple container's image store"
    "$CONTAINER" system start >/dev/null 2>&1 || true
    tmp="$(mktemp -t wechat-atspi)"  # BSD mktemp: arg is a prefix; a `.tar` suffix isn't needed (load reads the archive by content)
    "$DOCKER" save "$TAG" -o "$tmp"
    "$CONTAINER" image load -i "$tmp"
    rm -f "$tmp"
fi

echo
echo "[build] done -> $TAG (in the $RUNTIME image store)"
echo "Run the connector against it:  WECHAT_RUNTIME=$RUNTIME WECHAT_IMAGE=$TAG"
