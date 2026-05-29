#!/usr/bin/env bash
# cua-computer-sandbox.sh — Manage a CUA computer-use sandbox for Puffer.
#
# Brings up a containerised XFCE Linux desktop running CUA's computer-server
# (trycua/cua, MIT). The server exposes ~44 computer-use actions over an MCP
# streamable-HTTP endpoint at /mcp, which Puffer connects to as an external MCP
# server — Puffer's own model loop then drives the desktop (screenshot, click,
# type, …). See specs/puffer-core/118.md for the full integration contract.
#
# Usage:
#   ./scripts/cua-computer-sandbox.sh up        # pull (if needed) + run the sandbox
#   ./scripts/cua-computer-sandbox.sh down       # stop + remove the container
#   ./scripts/cua-computer-sandbox.sh status     # container state + /status health
#   ./scripts/cua-computer-sandbox.sh shot [out] # save a screenshot (default: /tmp/cua-shot.png)
#   ./scripts/cua-computer-sandbox.sh register   # print the `puffer mcp add` command
#
# Environment (override before running):
#   CUA_IMAGE      Docker image          (default: trycua/cua-xfce:latest)
#   CUA_NAME       Container name        (default: cua-sandbox)
#   CUA_API_PORT   computer-server port  (default: 8000)
#   CUA_VNC_PORT   noVNC web port        (default: 6901)  → http://localhost:$CUA_VNC_PORT
#   CUA_PLATFORM   Docker platform       (default: linux/amd64)
#
# Requires a running Docker daemon (OrbStack / Docker Desktop / colima).

set -euo pipefail

CUA_IMAGE="${CUA_IMAGE:-trycua/cua-xfce:latest}"
CUA_NAME="${CUA_NAME:-cua-sandbox}"
CUA_API_PORT="${CUA_API_PORT:-8000}"
CUA_VNC_PORT="${CUA_VNC_PORT:-6901}"
CUA_PLATFORM="${CUA_PLATFORM:-linux/amd64}"
MCP_URL="http://localhost:${CUA_API_PORT}/mcp/"

die() { echo "error: $*" >&2; exit 1; }

require_docker() {
  command -v docker >/dev/null 2>&1 || die "docker not found on PATH"
  docker info >/dev/null 2>&1 || die "docker daemon not running (start OrbStack / Docker Desktop)"
}

cmd_up() {
  require_docker
  if ! docker image inspect "$CUA_IMAGE" >/dev/null 2>&1; then
    echo "pulling $CUA_IMAGE (~4GB, one-time) …"
    docker pull --platform="$CUA_PLATFORM" "$CUA_IMAGE"
  fi
  docker rm -f "$CUA_NAME" >/dev/null 2>&1 || true
  echo "starting $CUA_NAME …"
  docker run -d --name "$CUA_NAME" --platform "$CUA_PLATFORM" \
    -p "${CUA_API_PORT}:8000" -p "${CUA_VNC_PORT}:6901" \
    -e VNC_PW=password -e VNCOPTIONS=-disableBasicAuth \
    "$CUA_IMAGE" >/dev/null
  echo -n "waiting for computer-server"
  for _ in $(seq 1 40); do
    if curl -fsS -m 3 "http://localhost:${CUA_API_PORT}/status" >/dev/null 2>&1; then
      echo " — ready."
      curl -fsS "http://localhost:${CUA_API_PORT}/status"; echo
      echo "watch the desktop at: http://localhost:${CUA_VNC_PORT}  (VNC_PW=password)"
      cmd_register
      return 0
    fi
    echo -n "."; sleep 2
  done
  die "computer-server did not become ready; check: docker logs $CUA_NAME"
}

cmd_down() {
  require_docker
  docker rm -f "$CUA_NAME" >/dev/null 2>&1 && echo "removed $CUA_NAME" || echo "$CUA_NAME not running"
}

cmd_status() {
  require_docker
  docker ps --filter "name=${CUA_NAME}" --format 'table {{.Names}}\t{{.Status}}\t{{.Ports}}' || true
  echo -n "health: "
  curl -fsS -m 3 "http://localhost:${CUA_API_PORT}/status" 2>/dev/null || echo "unreachable"
  echo
}

cmd_shot() {
  local out="${1:-/tmp/cua-shot.png}"
  curl -fsS -m 15 -X POST "http://localhost:${CUA_API_PORT}/cmd" \
    -H 'Content-Type: application/json' \
    -d '{"command":"screenshot","params":{}}' \
  | python3 -c "
import sys,json,base64
for line in sys.stdin:
    line=line.strip()
    if line.startswith('data:'):
        d=json.loads(line[5:].strip())
        if d.get('image_data'):
            open('$out','wb').write(base64.b64decode(d['image_data']))
            print('saved $out')
"
}

cmd_register() {
  cat <<EOF
register with Puffer (one-time):
  puffer mcp add cua-computer "${MCP_URL}" -t http -s user
then drive a turn, e.g.:
  puffer non-interactive --provider <vision-provider> --model <vision-model> \\
    --user-message "Take a screenshot and describe the desktop." --max-tool-calls 6
EOF
}

case "${1:-}" in
  up)       cmd_up ;;
  down)     cmd_down ;;
  status)   cmd_status ;;
  shot)     cmd_shot "${2:-}" ;;
  register) cmd_register ;;
  *) grep '^#' "$0" | sed 's/^# \{0,1\}//'; exit 1 ;;
esac
