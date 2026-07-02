#!/usr/bin/env bash
if [ -z "${BASH_VERSION:-}" ]; then
  if command -v bash >/dev/null 2>&1; then
    exec bash -s "$@"
  fi
  printf '%s\n' "Basse installer requires bash. Install bash, then run: curl -fsSL https://basse.sh/install | bash" >&2
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
SCRIPT_PATH="${BASH_SOURCE[0]-$0}"
SCRIPT_DIR="$(cd -- "$(dirname -- "$SCRIPT_PATH")" >/dev/null 2>&1 && pwd -P || printf '.')"

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
  printf '%s\n' "${bold}Self-host installer${reset}"
  printf '%s\n' "${dim}Installs the control plane under $INSTALL_DIR${reset}"
  printf '%s\n' ""
}

trim() {
  local value="$*"
  value="${value#"${value%%[![:space:]]*}"}"
  value="${value%"${value##*[![:space:]]}"}"
  printf '%s' "$value"
}

prompt() {
  local label="$1"
  local default="${2:-}"
  local value
  if [[ -n "$default" ]]; then
    read -r -p "$(printf '%s [%s]: ' "$label" "$default")" value
    printf '%s' "$(trim "${value:-$default}")"
  else
    read -r -p "$(printf '%s: ' "$label")" value
    printf '%s' "$(trim "$value")"
  fi
}

prompt_secret() {
  local label="$1"
  local value
  read -r -s -p "$(printf '%s: ' "$label")" value
  printf '\n' >&2
  printf '%s' "$value"
}

confirm() {
  local label="$1"
  local default="${2:-n}"
  local suffix="[y/N]"
  [[ "$default" == "y" ]] && suffix="[Y/n]"
  local value
  read -r -p "$label $suffix " value
  value="$(trim "${value:-$default}")"
  case "${value,,}" in
    y|yes) return 0 ;;
    *) return 1 ;;
  esac
}

generate_secret() {
  if command -v openssl >/dev/null 2>&1; then
    openssl rand -base64 32
    return
  fi
  dd if=/dev/urandom bs=32 count=1 2>/dev/null | base64
}

require_root() {
  if [[ "${EUID:-$(id -u)}" -eq 0 ]]; then
    return
  fi
  if [[ -f "$0" && -r "$0" ]] && command -v sudo >/dev/null 2>&1; then
    info "Elevating with sudo so $INSTALL_DIR can be created."
    exec sudo -E bash "$0" "$@"
  fi
  fail "Run this installer as root, or install sudo first."
}

detect_os() {
  [[ "$(uname -s)" == "Linux" ]] || fail "$APP_NAME self-hosting currently supports Linux servers only."

  local arch
  arch="$(uname -m)"
  case "$arch" in
    x86_64|amd64|aarch64|arm64) ;;
    *) fail "Unsupported CPU architecture: $arch. Use x86_64/amd64 or arm64/aarch64." ;;
  esac

  if [[ ! -r /etc/os-release ]]; then
    fail "Could not detect Linux distribution: /etc/os-release is missing."
  fi

  # shellcheck disable=SC1091
  . /etc/os-release
  local id="${ID:-unknown}"
  local like="${ID_LIKE:-}"
  case "$id $like" in
    *debian*|*ubuntu*|*fedora*|*rhel*|*centos*|*rocky*|*almalinux*|*arch*)
      ok "Detected ${PRETTY_NAME:-Linux} on $arch"
      ;;
    *)
      fail "Unsupported Linux distribution: ${PRETTY_NAME:-$id}. Supported families: Debian/Ubuntu, Fedora/RHEL/Rocky/Alma, Arch."
      ;;
  esac
}

install_base_tooling() {
  if command -v curl >/dev/null 2>&1 || command -v wget >/dev/null 2>&1; then
    return
  fi

  info "Installing curl..."
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    apt-get install -y ca-certificates curl
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y ca-certificates curl
  elif command -v yum >/dev/null 2>&1; then
    yum install -y ca-certificates curl
  elif command -v pacman >/dev/null 2>&1; then
    pacman -Sy --noconfirm ca-certificates curl
  else
    fail "Need curl or wget, and no supported package manager was found."
  fi
}

install_docker() {
  if command -v docker >/dev/null 2>&1; then
    return
  fi

  install_base_tooling
  info "Docker is not installed. Installing Docker Engine..."
  if command -v curl >/dev/null 2>&1; then
    curl -fsSL https://get.docker.com | sh
  elif command -v wget >/dev/null 2>&1; then
    wget -qO- https://get.docker.com | sh
  else
    fail "Need curl or wget to install Docker."
  fi
}

start_docker() {
  if docker info >/dev/null 2>&1; then
    return
  fi

  info "Starting Docker..."
  if command -v systemctl >/dev/null 2>&1; then
    systemctl enable --now docker
  elif command -v service >/dev/null 2>&1; then
    service docker start
  fi

  for _ in $(seq 1 20); do
    docker info >/dev/null 2>&1 && return
    sleep 1
  done

  fail "Docker is installed, but the daemon is not reachable."
}

install_compose_plugin() {
  if docker_compose_cmd >/dev/null 2>&1; then
    return
  fi

  info "Installing Docker Compose plugin..."
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    apt-get install -y docker-compose-plugin
  elif command -v dnf >/dev/null 2>&1; then
    dnf install -y docker-compose-plugin
  elif command -v yum >/dev/null 2>&1; then
    yum install -y docker-compose-plugin
  elif command -v pacman >/dev/null 2>&1; then
    pacman -Sy --noconfirm docker-compose
  else
    fail "Docker Compose is missing, and no supported package manager was found."
  fi
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

check_dependencies() {
  install_docker
  start_docker
  install_compose_plugin
  COMPOSE_CMD="$(docker_compose_cmd)" || fail "Docker Compose is not installed. Install the Docker Compose plugin, then re-run this script."
  export COMPOSE_CMD
  ok "Docker is ready"
}

validate_domain() {
  local domain="$1"
  [[ -n "$domain" ]] || return 1
  [[ "$domain" == "localhost" ]] && return 0
  [[ "$domain" =~ ^([A-Za-z0-9]([A-Za-z0-9-]{0,61}[A-Za-z0-9])?\.)+[A-Za-z]{2,63}$ ]]
}

write_caddyfile() {
  cat >"$CADDY_FILE" <<'EOF'
{$DOMAIN:localhost} {
	reverse_proxy control-plane:3000
}
EOF
}

install_compose_file() {
  if [[ -f "$SCRIPT_DIR/docker-compose.yml" && "$SCRIPT_DIR" != "$INSTALL_DIR" ]]; then
    cp "$SCRIPT_DIR/docker-compose.yml" "$COMPOSE_FILE"
    return
  fi

  if command -v curl >/dev/null 2>&1; then
    curl -fsSL "$RAW_BASE/docker-compose.yml" -o "$COMPOSE_FILE"
    return
  fi

  if command -v wget >/dev/null 2>&1; then
    wget -qO "$COMPOSE_FILE" "$RAW_BASE/docker-compose.yml"
    return
  fi

  fail "Need curl or wget to download docker-compose.yml."
}

install_update_file() {
  if [[ -f "$SCRIPT_DIR/update.sh" && "$SCRIPT_DIR" != "$INSTALL_DIR" ]]; then
    cp "$SCRIPT_DIR/update.sh" "$UPDATE_FILE"
  elif command -v curl >/dev/null 2>&1; then
    curl -fsSL "$RAW_BASE/update.sh" -o "$UPDATE_FILE"
  elif command -v wget >/dev/null 2>&1; then
    wget -qO "$UPDATE_FILE" "$RAW_BASE/update.sh"
  else
    fail "Need curl or wget to download update.sh."
  fi
  chmod +x "$UPDATE_FILE"
}

escape_env() {
  local value="$1"
  value="${value//$'\n'/}"
  value="${value//\\/\\\\}"
  value="${value//\"/\\\"}"
  printf '"%s"' "$value"
}

write_env_file() {
  local domain="$1"
  local public_url="$2"
  local auth_secret="$3"
  local postgres_password="$4"
  local email_verification="$5"
  local email_from="$6"
  local smtp_host="$7"
  local smtp_port="$8"
  local smtp_secure="$9"
  local smtp_require_tls="${10}"
  local smtp_allow_insecure="${11}"
  local smtp_auth_method="${12}"
  local smtp_user="${13}"
  local smtp_password="${14}"

  cat >"$ENV_FILE" <<EOF
DOMAIN=$domain
BASSE_DATA_DIR=$INSTALL_DIR
BASSE_PUBLIC_URL=$public_url
WEB_ORIGIN=$public_url
API_ORIGIN=$public_url
BETTER_AUTH_SECRET=$(escape_env "$auth_secret")
EMAIL_VERIFICATION=$email_verification
CONTROL_PLANE_IMAGE=ghcr.io/lassejlv/basse-control-plane:latest
BASSE_AGENT_IMAGE=ghcr.io/lassejlv/basse-agent:latest
HTTP_PORT=80
HTTPS_PORT=443
POSTGRES_DB=basse
POSTGRES_USER=basse
POSTGRES_PASSWORD=$(escape_env "$postgres_password")
QUEUE_CONCURRENCY=5
EMAIL_FROM=$(escape_env "$email_from")
SMTP_HOST=$(escape_env "$smtp_host")
SMTP_PORT=$smtp_port
SMTP_SECURE=$smtp_secure
SMTP_REQUIRE_TLS=$smtp_require_tls
SMTP_ALLOW_INSECURE_AUTH=$smtp_allow_insecure
SMTP_AUTH_METHOD=$smtp_auth_method
SMTP_USER=$(escape_env "$smtp_user")
SMTP_PASSWORD=$(escape_env "$smtp_password")
EOF
  chmod 600 "$ENV_FILE"
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

  warn "The stack started, but health did not settle in time. Recent logs:"
  $COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" logs --tail=80 control-plane || true
}

main() {
  banner
  require_root "$@"
  detect_os
  check_dependencies

  mkdir -p "$INSTALL_DIR"/{postgres,redis,caddy/data,caddy/config}

  if [[ -f "$ENV_FILE" || -f "$COMPOSE_FILE" ]]; then
    confirm "$INSTALL_DIR already contains a Basse install. Update configuration and restart?" "n" \
      || fail "Install cancelled."
  fi

  local domain=""
  while ! validate_domain "$domain"; do
    domain="$(prompt "Domain for Basse" "${DOMAIN:-}")"
    validate_domain "$domain" || warn "Enter a valid domain, for example basse.example.com"
  done

  local scheme="https"
  if [[ "$domain" == "localhost" ]]; then
    scheme="http"
  fi
  local public_url="$scheme://$domain"

  local configure_smtp="false"
  local smtp_host=""
  local smtp_port="587"
  local smtp_secure="false"
  local smtp_require_tls="false"
  local smtp_allow_insecure="false"
  local smtp_auth_method="plain"
  local smtp_user=""
  local smtp_password=""
  local email_from="Basse <alerts@$domain>"

  if confirm "Configure SMTP now? Email is optional." "n"; then
    configure_smtp="true"
    while [[ -z "$smtp_host" ]]; do
      smtp_host="$(prompt "SMTP host")"
      [[ -n "$smtp_host" ]] || warn "SMTP host is required when SMTP is enabled."
    done
    smtp_port="$(prompt "SMTP port" "587")"
    confirm "Use implicit TLS? Usually yes for port 465." "n" && smtp_secure="true"
    confirm "Require STARTTLS? Recommended for authenticated SMTP on port 587." "y" && smtp_require_tls="true"
    smtp_user="$(prompt "SMTP username (leave empty for no auth)")"
    if [[ -n "$smtp_user" ]]; then
      smtp_password="$(prompt_secret "SMTP password")"
      confirm "Use AUTH LOGIN instead of AUTH PLAIN?" "n" && smtp_auth_method="login"
    fi
    email_from="$(prompt "From address" "$email_from")"
  fi

  local email_verification="false"
  if [[ "$configure_smtp" == "true" ]]; then
    confirm "Require email verification for new signups?" "y" && email_verification="true"
  else
    warn "SMTP skipped. Email verification will stay disabled for first-run bootstrap."
  fi

  local auth_secret postgres_password
  auth_secret="$(generate_secret)"
  postgres_password="$(generate_secret)"

  install_compose_file
  install_update_file
  write_caddyfile
  write_env_file \
    "$domain" \
    "$public_url" \
    "$auth_secret" \
    "$postgres_password" \
    "$email_verification" \
    "$email_from" \
    "$smtp_host" \
    "$smtp_port" \
    "$smtp_secure" \
    "$smtp_require_tls" \
    "$smtp_allow_insecure" \
    "$smtp_auth_method" \
    "$smtp_user" \
    "$smtp_password"

  ok "Wrote $ENV_FILE"
  ok "Wrote $COMPOSE_FILE"
  ok "Wrote $UPDATE_FILE"
  ok "Data will live in $INSTALL_DIR"

  info "Starting Basse..."
  (cd "$INSTALL_DIR" && $COMPOSE_CMD --env-file "$ENV_FILE" -f "$COMPOSE_FILE" up -d)
  wait_for_health

  printf '%s\n' ""
  ok "$APP_NAME is installed"
  printf '%s\n' "${bold}URL:${reset} $public_url"
  printf '%s\n' "${bold}Directory:${reset} $INSTALL_DIR"
  printf '%s\n' "${bold}Manage:${reset} cd $INSTALL_DIR && $COMPOSE_CMD --env-file .env -f docker-compose.yml ps"
  printf '%s\n' "${bold}Update:${reset} $UPDATE_FILE"
  printf '%s\n' ""
}

main "$@"
