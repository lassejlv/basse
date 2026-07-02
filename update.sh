#!/usr/bin/env bash
if [ -z "${BASH_VERSION:-}" ]; then
  if command -v bash >/dev/null 2>&1; then
    exec bash -s "$@"
  fi
  printf '%s\n' "Basse updater requires bash. Install bash, then run: curl -fsSL https://basse.sh/update | bash" >&2
  exit 1
fi

set -Eeuo pipefail

APP_NAME="Basse"
INSTALL_DIR="${BASSE_INSTALL_DIR:-/data/basse}"
RAW_BASE="${BASSE_INSTALL_RAW_BASE:-https://raw.githubusercontent.com/lassejlv/basse/main}"
COMPOSE_FILE="$INSTALL_DIR/docker-compose.yml"
ENV_FILE="$INSTALL_DIR/.env"
CADDY_FILE="$INSTALL_DIR/Caddyfile"
UPDATE_FILE="$INSTALL_DIR/update.sh"

if [[ -t 1 ]]; then
  bold="$(printf '\033[1m')"
  dim="$(printf '\033[2m')"
  red="$(printf '\033[31m')"
  green="$(printf '\033[32m')"
  yellow="$(printf '\033[33m')"
  blue="$(printf '\033[34m')"
  cyan="$(printf '\033[36m')"
  reset="$(printf '\033[0m')"
else
  bold=""
  dim=""
  red=""
  green=""
  yellow=""
  blue=""
  cyan=""
  reset=""
fi

info() {
  printf '%s\n' "${blue}==>${reset} $*"
}

ok() {
  printf '%s\n' "${green}OK${reset} $*"
}

warn() {
  printf '%s\n' "${yellow}!${reset} $*"
}

fail() {
  printf '%s\n' "${red}ERROR${reset} $*" >&2
  exit 1
}

banner() {
  printf '%s\n' ""
  printf '%s\n' "${cyan}${bold}  ____                       ${reset}"
  printf '%s\n' "${cyan}${bold} | __ )  __ _ ___ ___  ___   ${reset}"
  printf '%s\n' "${cyan}${bold} |  _ \\ / _\` / __/ __|/ _ \\  ${reset}"
  printf '%s\n' "${cyan}${bold} | |_) | (_| \\__ \\__ \\  __/  ${reset}"
  printf '%s\n' "${cyan}${bold} |____/ \\__,_|___/___/\\___|  ${reset}"
  printf '%s\n' ""
  printf '%s\n' "${bold}Self-host updater${reset}"
  printf '%s\n' "${dim}Updates the stack in $INSTALL_DIR${reset}"
  printf '%s\n' ""
}

require_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    return
  fi
  if [[ -f "$0" && -r "$0" ]] && command -v sudo >/dev/null 2>&1; then
    info "Elevating with sudo so $INSTALL_DIR can be updated."
    exec sudo -E bash "$0" "$@"
  fi
  fail "Run this updater as root, or install sudo first."
}

docker_compose_cmd() {
  if docker compose version >/dev/null 2>&1; then
    printf 'docker compose'
    return
  fi
  if command -v docker-compose >/dev/null 2>&1; then
    printf 'docker-compose'
    return
  fi
  return 1
}

download() {
  local url="$1"
  local destination="$2"
  local tmp
  tmp="$(mktemp)"
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$url" -o "$tmp"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$tmp" "$url"
  else
    rm -f "$tmp"
    fail "Need curl or wget to download update files."
  fi
  mv "$tmp" "$destination"
}

write_caddyfile() {
  cat >"$CADDY_FILE" <<'EOF'
{$DOMAIN:localhost} {
	reverse_proxy control-plane:3000
}
EOF
}

is_cloud_managed() {
  env | grep -q '^CLOUD_' && return 0
  [[ -f "$ENV_FILE" ]] && grep -Eq '^[[:space:]]*CLOUD_[A-Za-z0-9_]*=' "$ENV_FILE"
}

wait_for_health() {
  info "Waiting for the control plane to become healthy..."
  local container_id
  for _ in $(seq 1 90); do
    container_id="$($COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" ps -q control-plane 2>/dev/null || true)"
    if [[ -n "$container_id" ]]; then
      local status
      status="$(docker inspect -f '{{if .State.Health}}{{.State.Health.Status}}{{else}}{{.State.Status}}{{end}}' "$container_id" 2>/dev/null || true)"
      if [[ "$status" == "healthy" || "$status" == "running" ]]; then
        ok "Control plane is $status"
        return
      fi
    fi
    sleep 2
  done

  warn "The stack restarted, but health did not settle in time. Recent logs:"
  $COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" logs --tail=80 control-plane || true
}

main() {
  banner
  require_root "$@"

  [[ "$(uname -s)" == "Linux" ]] || fail "$APP_NAME self-host updates currently support Linux servers only."
  [[ -f "$ENV_FILE" ]] || fail "Missing $ENV_FILE. Run the installer first."
  [[ -f "$COMPOSE_FILE" ]] || fail "Missing $COMPOSE_FILE. Run the installer first."
  command -v docker >/dev/null 2>&1 || fail "Docker is not installed."
  docker info >/dev/null 2>&1 || fail "Docker daemon is not reachable."
  COMPOSE_CMD="$(docker_compose_cmd)" || fail "Docker Compose is not installed."
  export COMPOSE_CMD

  if is_cloud_managed; then
    fail "This instance looks cloud-managed because a CLOUD_ env variable is set. Use the cloud deploy pipeline instead."
  fi

  local backup_dir="$INSTALL_DIR/backups/update-$(date -u +%Y%m%d%H%M%S)"
  mkdir -p "$backup_dir"
  cp "$ENV_FILE" "$backup_dir/.env"
  cp "$COMPOSE_FILE" "$backup_dir/docker-compose.yml"
  [[ -f "$CADDY_FILE" ]] && cp "$CADDY_FILE" "$backup_dir/Caddyfile"
  ok "Backed up current config to $backup_dir"

  info "Refreshing compose and updater files..."
  download "$RAW_BASE/docker-compose.yml" "$COMPOSE_FILE"
  download "$RAW_BASE/update.sh" "$UPDATE_FILE"
  chmod +x "$UPDATE_FILE"
  write_caddyfile

  info "Pulling latest images..."
  (cd "$INSTALL_DIR" && $COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" pull)

  info "Restarting Basse..."
  (cd "$INSTALL_DIR" && $COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" up -d --remove-orphans)
  wait_for_health

  info "Cleaning unused image layers..."
  docker image prune -f >/dev/null 2>&1 || true

  printf '%s\n' ""
  ok "$APP_NAME is updated"
  printf '%s\n' "${bold}Backup:${reset} $backup_dir"
  printf '%s\n' "${bold}Status:${reset} cd $INSTALL_DIR && $COMPOSE_CMD --env-file .env -f docker-compose.yml ps"
  printf '%s\n' ""
}

main "$@"
