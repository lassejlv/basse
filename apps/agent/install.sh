#!/bin/sh
set -eu

AGENT_NAME="${BASSE_AGENT_CONTAINER:-basse-agent}"
AGENT_IMAGE="${BASSE_AGENT_IMAGE:-ghcr.io/lassejlv/basse-agent:latest}"
AGENT_MODE="${BASSE_AGENT_MODE:-outbound}"
AGENT_PORT="${BASSE_AGENT_PORT:-8888}"
CADDY_DATA_VOLUME="${BASSE_CADDY_DATA_VOLUME:-basse_caddy_data}"
CADDY_ADMIN_VOLUME="${BASSE_CADDY_ADMIN_VOLUME:-basse_caddy_admin}"
CADDY_ADMIN_DIR="${BASSE_CADDY_ADMIN_DIR:-/run/caddy-admin}"

log() {
  printf '%s\n' "$*"
}

fail() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

need() {
  command -v "$1" >/dev/null 2>&1
}

as_root() {
  if [ "$(id -u)" -eq 0 ]; then
    "$@"
  elif need sudo; then
    sudo "$@"
  else
    fail "this step needs root. Re-run as root or install sudo."
  fi
}

docker_cmd() {
  if docker info >/dev/null 2>&1; then
    docker "$@"
  elif need sudo && sudo docker info >/dev/null 2>&1; then
    sudo docker "$@"
  else
    fail "Docker is installed but this user cannot talk to it. Run as root or add the user to the docker group."
  fi
}

install_docker() {
  if need docker; then
    return
  fi

  uname_s="$(uname -s 2>/dev/null || true)"
  if [ "$uname_s" != "Linux" ]; then
    fail "Docker is not installed. Install Docker manually on this OS, then rerun this script."
  fi
  if ! need curl; then
    fail "curl is required to install Docker automatically"
  fi

  log "Installing Docker..."
  tmp="$(mktemp)"
  curl -fsSL https://get.docker.com -o "$tmp"
  as_root sh "$tmp"
  rm -f "$tmp"

  if command -v systemctl >/dev/null 2>&1; then
    as_root systemctl enable --now docker >/dev/null 2>&1 || true
  elif command -v service >/dev/null 2>&1; then
    as_root service docker start >/dev/null 2>&1 || true
  fi
}

validate_config() {
  [ -n "${BASSE_AGENT_TOKEN:-}" ] || fail "BASSE_AGENT_TOKEN is required"
  [ "$AGENT_MODE" = "outbound" ] || fail "this installer is for outbound mode only"
  [ -n "${BASSE_CONTROL_PLANE_URL:-}" ] || fail "BASSE_CONTROL_PLANE_URL is required"

  case "$BASSE_CONTROL_PLANE_URL" in
    http://*|https://*) ;;
    *) fail "BASSE_CONTROL_PLANE_URL must start with http:// or https://" ;;
  esac
}

start_agent() {
  log "Preparing Docker volumes..."
  docker_cmd volume create "$CADDY_DATA_VOLUME" >/dev/null
  docker_cmd volume create "$CADDY_ADMIN_VOLUME" >/dev/null

  if docker_cmd ps -a --format '{{.Names}}' | grep -Fx "$AGENT_NAME" >/dev/null 2>&1; then
    log "Replacing existing $AGENT_NAME container..."
    docker_cmd rm -f "$AGENT_NAME" >/dev/null
  fi

  log "Pulling $AGENT_IMAGE..."
  docker_cmd pull "$AGENT_IMAGE" >/dev/null

  log "Starting $AGENT_NAME in outbound mode..."
  docker_cmd run -d \
    --name "$AGENT_NAME" \
    --restart unless-stopped \
    -v /var/run/docker.sock:/var/run/docker.sock \
    -v "$CADDY_DATA_VOLUME":/data \
    -v "$CADDY_ADMIN_VOLUME":"$CADDY_ADMIN_DIR" \
    -e BASSE_AGENT_TOKEN="$BASSE_AGENT_TOKEN" \
    -e BASSE_AGENT_MODE=outbound \
    -e BASSE_CONTROL_PLANE_URL="$BASSE_CONTROL_PLANE_URL" \
    -e BASSE_AGENT_PORT="$AGENT_PORT" \
    -e BASSE_CADDY_DATA_VOLUME="$CADDY_DATA_VOLUME" \
    -e BASSE_CADDY_ADMIN_VOLUME="$CADDY_ADMIN_VOLUME" \
    -e BASSE_CADDY_ADMIN_DIR="$CADDY_ADMIN_DIR" \
    "$AGENT_IMAGE" >/dev/null
}

verify_agent() {
  sleep 2
  if ! docker_cmd ps --filter "name=^/${AGENT_NAME}$" --format '{{.Names}}' | grep -Fx "$AGENT_NAME" >/dev/null 2>&1; then
    docker_cmd logs --tail 80 "$AGENT_NAME" >&2 || true
    fail "$AGENT_NAME did not stay running"
  fi

  log "$AGENT_NAME is running."
  log "It will become active in Basse after the first outbound poll succeeds."
  log "Useful checks:"
  log "  docker logs --tail 100 $AGENT_NAME"
  log "  docker ps --filter name=$AGENT_NAME"
}

validate_config
install_docker
start_agent
verify_agent
