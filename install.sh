#!/bin/sh
# Installs the control daemon on a Linux server: Docker, the daemon as a
# service, the reverse proxy, firewall ports and a logged-in CLI.
# With --cli it installs only the CLI, on Linux or macOS.
#
#   curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/install.sh | sudo sh
#   curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/install.sh | sh -s -- --cli
#
# Run with --help for every option. Safe to re-run: an existing install is upgraded in place.
# --uninstall removes it again; uninstall.sh is a shortcut for that.

set -eu

# Product names. Keep in sync with crates/common/src/brand.rs.
NAME="Basse"
CLI_BIN="basse"
DAEMON_BIN="bassed"
DIR_NAME="basse"
PREFIX="basse"
ENV_PREFIX="BASSE"
REPO="lassejlv/basse"
PORT=7700

DATA_DIR="/var/lib/$DIR_NAME"
BIN_DIR="/usr/local/bin"
UNIT="$DAEMON_BIN.service"

MODE="server"
VERSION="latest"
RUNNING_AS="running"
PROXY_STATE="off (--no-proxy)"
FROM_SOURCE=0
ARCHIVE=""
FIREWALL=1
PROXY=1
PUBLIC_IP=""
UNINSTALL=0
PURGE=0
VOLUMES=0
KEEP_CONFIG=0
YES=0
TOKEN="${GITHUB_TOKEN:-${GH_TOKEN:-}}"

usage() {
	cat <<EOF
$NAME installer

Usage: install.sh [options]

  (no options)          Install or upgrade the server: Docker, $DAEMON_BIN, proxy, $CLI_BIN
  --cli                 Install only the $CLI_BIN CLI (Linux or macOS), e.g. on your laptop
  --version <tag>       Release to install, e.g. v0.2.0 [default: latest]
  --from-source         Build with cargo instead of downloading a release
  --archive <file>      Install binaries from a local release archive
  --public-ip <ip>      Address shown in login hints [default: detected]
  --no-firewall         Leave ufw/firewalld alone
  --no-proxy            Don't run the reverse proxy on this server
  --github-token <t>    Token for a private repository [env: GITHUB_TOKEN]
  --uninstall           Remove the daemon, its service and the binaries; keep data and apps
  --uninstall --purge   Also clean up every remote server, then remove app containers, the
                        proxy, networks, $DATA_DIR and the CLI logins on this server
  --volumes             With --purge: also delete the app volumes. This deletes app data.
  --uninstall --cli     Remove only the $CLI_BIN CLI and its saved logins (~/.config/$DIR_NAME)
  --keep-config         With --uninstall: keep the CLI's saved logins
  -y, --yes             Don't ask for confirmation
  -h, --help            Show this help
EOF
}

while [ $# -gt 0 ]; do
	case "$1" in
	--cli) MODE="cli" ;;
	--version) VERSION="$2"; shift ;;
	--from-source) FROM_SOURCE=1 ;;
	--archive) ARCHIVE="$2"; shift ;;
	--public-ip) PUBLIC_IP="$2"; shift ;;
	--no-firewall) FIREWALL=0 ;;
	--no-proxy) PROXY=0 ;;
	--github-token) TOKEN="$2"; shift ;;
	--uninstall) UNINSTALL=1 ;;
	--purge) PURGE=1 ;;
	--volumes) VOLUMES=1 ;;
	--keep-config) KEEP_CONFIG=1 ;;
	-y | --yes) YES=1 ;;
	-h | --help) usage; exit 0 ;;
	*) echo "unknown option: $1 (see --help)" >&2; exit 2 ;;
	esac
	shift
done

# Output

if [ -t 1 ] && [ -z "${NO_COLOR:-}" ]; then
	C_ACCENT=$(printf '\033[38;2;245;165;36m')
	C_OK=$(printf '\033[38;2;79;191;127m')
	C_ERR=$(printf '\033[38;2;229;83;75m')
	C_MUTED=$(printf '\033[38;2;124;135;143m')
	C_RESET=$(printf '\033[0m')
else
	C_ACCENT="" C_OK="" C_ERR="" C_MUTED="" C_RESET=""
fi

step() { printf '%s==>%s %s\n' "$C_ACCENT" "$C_RESET" "$*"; }
ok() { printf '%s✓%s %s\n' "$C_OK" "$C_RESET" "$*"; }
note() { printf '%s    %s%s\n' "$C_MUTED" "$*" "$C_RESET"; }
warn() { printf '%s!%s %s\n' "$C_ACCENT" "$C_RESET" "$*" >&2; }
die() { printf '%serror:%s %s\n' "$C_ERR" "$C_RESET" "$*" >&2; exit 1; }
has() { command -v "$1" >/dev/null 2>&1; }

TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT INT TERM

# Platform

detect_target() {
	os=$(uname -s)
	arch=$(uname -m)
	case "$arch" in
	x86_64 | amd64) arch="x86_64" ;;
	aarch64 | arm64) arch="aarch64" ;;
	*) die "unsupported CPU architecture: $arch" ;;
	esac
	case "$os" in
	Linux) TARGET="$arch-unknown-linux-musl" ;;
	Darwin) TARGET="$arch-apple-darwin" ;;
	*) die "unsupported OS: $os" ;;
	esac
}

pkg_install() {
	if has apt-get; then
		DEBIAN_FRONTEND=noninteractive apt-get update -qq >/dev/null
		DEBIAN_FRONTEND=noninteractive apt-get install -y -qq "$@" >/dev/null
	elif has dnf; then
		dnf install -y -q "$@" >/dev/null
	elif has yum; then
		yum install -y -q "$@" >/dev/null
	elif has apk; then
		apk add --no-cache -q "$@" >/dev/null
	elif has zypper; then
		zypper --non-interactive --quiet install "$@" >/dev/null
	elif has pacman; then
		pacman -Sy --noconfirm --needed "$@" >/dev/null
	else
		die "no supported package manager found; install $* yourself and re-run"
	fi
}

ensure_tools() {
	missing=""
	has curl || missing="$missing curl"
	has git || missing="$missing git"
	has tar || missing="$missing tar"
	if ! has ssh-keygen; then
		if has apt-get; then
			missing="$missing openssh-client"
		elif has dnf || has yum; then
			missing="$missing openssh-clients"
		else
			missing="$missing openssh"
		fi
	fi
	[ -e /etc/ssl/certs/ca-certificates.crt ] || [ -e /etc/pki/tls/certs/ca-bundle.crt ] || missing="$missing ca-certificates"
	if [ -n "$missing" ]; then
		step "Installing$missing"
		# shellcheck disable=SC2086
		pkg_install $missing
	fi
}

has_systemd() { [ -d /run/systemd/system ]; }

# Docker

ensure_docker() {
	if ! has docker; then
		step "Installing Docker"
		if has apk; then
			pkg_install docker
			if has rc-update; then rc-update add docker default >/dev/null 2>&1 || true; fi
		elif has pacman; then
			pkg_install docker
		else
			curl -fsSL https://get.docker.com -o "$TMP/get-docker.sh"
			sh "$TMP/get-docker.sh" >"$TMP/docker-install.log" 2>&1 || {
				tail -20 "$TMP/docker-install.log" >&2
				die "Docker installation failed"
			}
		fi
	fi
	if ! docker info >/dev/null 2>&1; then
		step "Starting Docker"
		if has_systemd; then
			systemctl enable --now docker >/dev/null 2>&1 || true
		elif has rc-service; then
			rc-service docker start >/dev/null 2>&1 || true
		elif has service; then
			service docker start >/dev/null 2>&1 || true
		fi
		if ! docker info >/dev/null 2>&1 && has dockerd; then
			nohup dockerd >/var/log/dockerd.log 2>&1 &
		fi
		i=0
		until docker info >/dev/null 2>&1; do
			i=$((i + 1))
			[ $i -gt 60 ] && die "Docker did not start; check 'docker info'"
			sleep 1
		done
	fi
	ok "Docker $(docker version --format '{{.Server.Version}}' 2>/dev/null)"
}

# Binaries

github_curl() {
	if [ -n "$TOKEN" ]; then
		curl -fsSL -H "Authorization: Bearer $TOKEN" "$@"
	else
		curl -fsSL "$@"
	fi
}

# Downloads a release asset to $2; fails quietly so the caller can fall back to a source build.
download_asset() {
	dl_name="$1"
	dl_out="$2"
	if [ -z "$TOKEN" ]; then
		if [ "$VERSION" = latest ]; then
			dl_url="https://github.com/$REPO/releases/latest/download/$dl_name"
		else
			dl_url="https://github.com/$REPO/releases/download/$VERSION/$dl_name"
		fi
		curl -fsSL "$dl_url" -o "$dl_out" 2>/dev/null
		return
	fi
	# Private repositories only serve assets through the API, by asset id.
	if [ "$VERSION" = latest ]; then
		dl_api="https://api.github.com/repos/$REPO/releases/latest"
	else
		dl_api="https://api.github.com/repos/$REPO/releases/tags/$VERSION"
	fi
	github_curl "$dl_api" -o "$TMP/release.json" 2>/dev/null || return 1
	dl_asset_url=$(awk -v name="\"$dl_name\"" '
		/"url": "https:\/\/api.github.com\/repos\/.*\/releases\/assets\// { gsub(/[",]/, "", $2); url = $2 }
		/"name":/ { n = $2; gsub(/,$/, "", n); if (n == name) { print url; exit } }
	' "$TMP/release.json")
	[ -n "$dl_asset_url" ] || return 1
	github_curl -H "Accept: application/octet-stream" "$dl_asset_url" -o "$dl_out" 2>/dev/null
}

verify_checksum() {
	file="$1"
	sum_file="$2"
	if has sha256sum; then
		actual=$(sha256sum "$file" | cut -d' ' -f1)
	elif has shasum; then
		actual=$(shasum -a 256 "$file" | cut -d' ' -f1)
	else
		warn "no sha256 tool found; skipping checksum verification"
		return 0
	fi
	expected=$(cut -d' ' -f1 "$sum_file")
	[ "$actual" = "$expected" ] || die "checksum mismatch for $(basename "$file")"
}

# Installs $@ (binary names) into $INSTALL_DIR from a release, a local archive or a source build.
install_binaries() {
	mkdir -p "$TMP/bin"
	if [ -n "$ARCHIVE" ]; then
		step "Installing from $ARCHIVE"
		tar -xzf "$ARCHIVE" -C "$TMP/bin"
	elif [ "$FROM_SOURCE" = 0 ] && fetch_release; then
		:
	else
		build_from_source "$@"
	fi
	mkdir -p "$INSTALL_DIR"
	for bin in "$@"; do
		[ -f "$TMP/bin/$bin" ] || die "$bin is missing from the release archive"
		install -m 0755 "$TMP/bin/$bin" "$INSTALL_DIR/$bin.new"
		mv -f "$INSTALL_DIR/$bin.new" "$INSTALL_DIR/$bin"
	done
	ok "Installed $* to $INSTALL_DIR ($("$INSTALL_DIR/$1" --version | awk '{print $2}'))"
}

fetch_release() {
	archive="$CLI_BIN-$TARGET.tar.gz"
	step "Downloading $NAME $VERSION for $TARGET"
	if ! download_asset "$archive" "$TMP/$archive"; then
		warn "no release download available for $TARGET; building from source instead"
		return 1
	fi
	if download_asset "$archive.sha256" "$TMP/$archive.sha256"; then
		verify_checksum "$TMP/$archive" "$TMP/$archive.sha256"
	fi
	tar -xzf "$TMP/$archive" -C "$TMP/bin"
}

build_from_source() {
	step "Building from source (takes a few minutes)"
	if ! has cargo; then
		if [ "$(uname -s)" = Linux ] && [ "$(id -u)" = 0 ]; then
			if has apt-get; then pkg_install build-essential pkg-config
			elif has dnf || has yum; then pkg_install gcc gcc-c++ make
			elif has apk; then pkg_install build-base
			elif has pacman; then pkg_install base-devel
			fi
		fi
		note "installing a Rust toolchain with rustup"
		curl -fsSL https://sh.rustup.rs | sh -s -- -y -q --profile minimal >/dev/null
		# shellcheck disable=SC1091
		. "$HOME/.cargo/env"
	fi
	url="https://github.com/$REPO"
	[ -n "$TOKEN" ] && url="https://x-access-token:$TOKEN@github.com/$REPO"
	ref="--branch main"
	[ "$VERSION" != latest ] && ref="--tag $VERSION"
	packages="cli"
	[ "$MODE" = server ] && packages="daemon cli"
	# shellcheck disable=SC2086
	cargo install --quiet --locked --git "$url" $ref --root "$TMP/cargo" $packages ||
		die "source build failed"
	cp "$TMP/cargo/bin/"* "$TMP/bin/"
}

# Server

public_ip() {
	[ -n "$PUBLIC_IP" ] && { echo "$PUBLIC_IP"; return; }
	ip=$(curl -fsS -4 --max-time 4 https://api.ipify.org 2>/dev/null || true)
	[ -z "$ip" ] && has hostname && ip=$(hostname -I 2>/dev/null | awk '{print $1}')
	[ -z "$ip" ] && ip="<server-ip>"
	echo "$ip"
}

check_ports() {
	has ss || return 0
	for p in 80 443 "$PORT"; do
		holder=$(ss -ltnpH "sport = :$p" 2>/dev/null | grep -v -e docker-proxy -e "$DAEMON_BIN" | head -1 || true)
		if [ -n "$holder" ]; then
			warn "port $p is already in use: $(echo "$holder" | sed 's/.*users:((//; s/,.*//')"
			[ "$p" = "$PORT" ] || note "the proxy needs ports 80 and 443 to serve your domains"
		fi
	done
}

open_firewall() {
	[ "$FIREWALL" = 1 ] || return 0
	if has ufw && ufw status 2>/dev/null | grep -q "Status: active"; then
		for p in 80 443 "$PORT"; do ufw allow "$p/tcp" >/dev/null; done
		ufw allow 443/udp >/dev/null
		ok "Opened ports 80, 443 and $PORT in ufw"
	elif has firewall-cmd && firewall-cmd --state >/dev/null 2>&1; then
		for p in 80/tcp 443/tcp 443/udp "$PORT/tcp"; do firewall-cmd --quiet --permanent --add-port="$p"; done
		firewall-cmd --quiet --reload
		ok "Opened ports 80, 443 and $PORT in firewalld"
	fi
}

start_daemon() {
	if has_systemd; then
		"$BIN_DIR/$DAEMON_BIN" --data-dir "$DATA_DIR" install-service >/dev/null
		if [ -n "$PUBLIC_IP" ]; then
			mkdir -p "/etc/systemd/system/$UNIT.d"
			printf '[Service]\nEnvironment=%s_PUBLIC_IP=%s\n' "$ENV_PREFIX" "$PUBLIC_IP" >"/etc/systemd/system/$UNIT.d/public-ip.conf"
		fi
		systemctl daemon-reload
		systemctl enable "$UNIT" >/dev/null 2>&1
		systemctl restart "$UNIT"
		ok "$DAEMON_BIN is running under systemd ($UNIT)"
		RUNNING_AS="systemd ($UNIT)"
	else
		warn "systemd not found: starting $DAEMON_BIN in the background; add it to your init system to survive reboots"
		RUNNING_AS="in the background, logs in /var/log/$DAEMON_BIN.log"
		pkill -x "$DAEMON_BIN" 2>/dev/null || true
		env "${ENV_PREFIX}_PUBLIC_IP=$PUBLIC_IP" nohup "$BIN_DIR/$DAEMON_BIN" --data-dir "$DATA_DIR" serve >"/var/log/$DAEMON_BIN.log" 2>&1 &
		note "logs: /var/log/$DAEMON_BIN.log"
	fi
	i=0
	until curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; do
		i=$((i + 1))
		if [ $i -gt 30 ]; then
			has_systemd && journalctl -u "$UNIT" -n 20 --no-pager >&2
			die "$DAEMON_BIN did not come up on port $PORT"
		fi
		sleep 1
	done
}

wait_for_proxy() {
	[ "$PROXY" = 1 ] || return 0
	step "Starting the reverse proxy"
	i=0
	until [ "$(docker inspect -f '{{.State.Running}}' "$PREFIX-proxy" 2>/dev/null)" = true ]; do
		i=$((i + 1))
		if [ $i -gt 120 ]; then
			warn "the proxy is not running yet; check '$CLI_BIN status' in a minute"
			PROXY_STATE="starting, check '$CLI_BIN status' in a minute"
			return 0
		fi
		sleep 1
	done
	ok "Proxy running on ports 80 and 443"
	PROXY_STATE="running on ports 80 and 443"
}

# Logs in the CLI for root and the user who ran sudo, so '$CLI_BIN' works right away.
login_local_cli() {
	token="$1"
	ctx=$(hostname 2>/dev/null || echo local)
	# sudo can keep the caller's XDG_CONFIG_HOME; drop it so root's config lands in /root.
	env -u XDG_CONFIG_HOME -u "${ENV_PREFIX}_CONFIG" HOME=/root \
		"$BIN_DIR/$CLI_BIN" login "http://127.0.0.1:$PORT" --token "$token" --name "$ctx" >/dev/null 2>&1 || true
	if [ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ] && has sudo; then
		sudo -u "$SUDO_USER" -H "$BIN_DIR/$CLI_BIN" login "http://127.0.0.1:$PORT" --token "$token" --name "$ctx" >/dev/null 2>&1 ||
			warn "could not log in the CLI for $SUDO_USER; run '$CLI_BIN login http://127.0.0.1:$PORT --token <token>' as $SUDO_USER"
	fi
}

install_server() {
	[ "$(uname -s)" = Linux ] || die "the daemon runs on Linux; use --cli to install only the CLI here"
	[ "$(id -u)" = 0 ] || die "run as root, e.g. 'curl -fsSL … | sudo sh'"
	detect_target
	printf '\n%s%s%s server install\n\n' "$C_ACCENT" "$NAME" "$C_RESET"
	ensure_tools
	ensure_docker
	check_ports
	INSTALL_DIR="$BIN_DIR"
	upgrading=0
	[ -f "$DATA_DIR/config.toml" ] && upgrading=1
	install_binaries "$DAEMON_BIN" "$CLI_BIN"

	token=""
	if [ "$upgrading" = 0 ]; then
		step "Initializing $DATA_DIR"
		set -- --data-dir "$DATA_DIR" init --token-file "$TMP/token"
		[ "$PROXY" = 0 ] && set -- "$@" --no-proxy
		"$BIN_DIR/$DAEMON_BIN" "$@" >/dev/null
		token=$(cat "$TMP/token")
	else
		note "existing install found in $DATA_DIR: data and token are kept"
	fi

	start_daemon
	wait_for_proxy
	open_firewall
	[ -n "$token" ] && login_local_cli "$token"

	summary "$(public_ip)" "$token"
}

# The last thing the installer prints: what runs, how to connect, what to do next.
summary() {
	ip="$1"
	token="$2"
	version=$("$BIN_DIR/$DAEMON_BIN" --version | awk '{print $2}')
	if [ -n "$token" ]; then
		printf '\n%s%s is ready%s\n\n' "$C_OK" "$NAME" "$C_RESET"
	else
		printf '\n%s%s is upgraded%s, data and token kept\n\n' "$C_OK" "$NAME" "$C_RESET"
	fi
	printf '  %-8s %s %s, %s\n' "daemon" "$DAEMON_BIN" "$version" "$RUNNING_AS"
	printf '  %-8s %s\n' "proxy" "$PROXY_STATE"
	printf '  %-8s %s\n' "docker" "$(docker version --format '{{.Server.Version}}' 2>/dev/null || echo running)"
	printf '  %-8s http://%s:%s\n' "api" "$ip" "$PORT"
	if [ -n "$token" ]; then
		printf '\n  On your laptop (macOS or Linux):\n\n'
		printf '    curl -fsSL https://raw.githubusercontent.com/%s/main/install.sh | sh -s -- --cli\n' "$REPO"
		printf '    %s%s login http://%s:%s --token %s%s\n\n' "$C_ACCENT" "$CLI_BIN" "$ip" "$PORT" "$token" "$C_RESET"
		note "the token is shown only once; '$DAEMON_BIN token rotate' issues a new one"
	fi
	printf '\n  Next: run %s%s%s, here or on your laptop, to set up this instance.\n\n' "$C_ACCENT" "$CLI_BIN" "$C_RESET"
}

install_cli() {
	detect_target
	if [ "$(id -u)" = 0 ]; then
		INSTALL_DIR="$BIN_DIR"
	else
		INSTALL_DIR="$HOME/.local/bin"
	fi
	install_binaries "$CLI_BIN"
	case ":$PATH:" in
	*":$INSTALL_DIR:"*) ;;
	*) warn "$INSTALL_DIR is not on your PATH; add it to your shell profile" ;;
	esac
	printf '\n  Connect with: %s%s login http://<server-ip>:%s --token <token>%s\n' "$C_ACCENT" "$CLI_BIN" "$PORT" "$C_RESET"
	printf '  Then run %s%s%s.\n\n' "$C_ACCENT" "$CLI_BIN" "$C_RESET"
}

# Uninstall

# Asks before removing anything when a terminal is attached, also under 'curl … | sh'. Without
# one (CI, provisioning) it goes ahead, as with --yes.
confirm() {
	printf '%s\n' "$1"
	[ "$YES" = 1 ] && return 0
	( : </dev/tty ) 2>/dev/null || return 0
	printf 'Continue? [y/N] '
	read -r answer </dev/tty || answer=""
	case "$answer" in
	y | Y | yes) ;;
	*) die "cancelled" ;;
	esac
}

REMOVED=""
removed() {
	REMOVED="$REMOVED
  - $*"
}

print_removed() {
	if [ -z "$REMOVED" ]; then
		note "nothing to remove"
	else
		printf '%sRemoved:%s%s\n' "$C_OK" "$C_RESET" "$REMOVED"
	fi
}

# Home directory of the user who ran sudo, if any.
sudo_home() {
	[ -n "${SUDO_USER:-}" ] && [ "$SUDO_USER" != root ] || return 0
	getent passwd "$SUDO_USER" 2>/dev/null | cut -d: -f6
}

# Runs the CLI as root, logged in to this server's daemon by the installer.
root_cli() {
	env -u XDG_CONFIG_HOME -u "${ENV_PREFIX}_CONFIG" HOME=/root "$BIN_DIR/$CLI_BIN" "$@"
}

# Asks the running daemon to remove what it put on every SSH server, before the daemon goes away.
# An unreachable server only warns: the CLI prints the command to clean it up by hand.
clean_remote_servers() {
	if [ ! -x "$BIN_DIR/$CLI_BIN" ] || ! curl -fsS "http://127.0.0.1:$PORT/healthz" >/dev/null 2>&1; then
		warn "the daemon is not running, so remote servers were not cleaned up"
		return 0
	fi
	step "Cleaning up remote servers"
	set -- server remove --all --purge --yes
	[ "$VOLUMES" = 1 ] && set -- "$@" --volumes
	if root_cli "$@"; then
		removed "what $DAEMON_BIN put on its remote servers"
	else
		warn "not every remote server could be cleaned up; see the commands above"
	fi
}

uninstall_server() {
	[ "$(id -u)" = 0 ] || die "run as root, e.g. 'curl -fsSL … | sudo sh -s -- --uninstall'"
	plan="This removes $DAEMON_BIN ($UNIT) and the $DAEMON_BIN and $CLI_BIN binaries from this server."
	if [ "$PURGE" = 1 ]; then
		plan="$plan
It also cleans up every remote server, then removes all app containers, the proxy, the
networks, $DATA_DIR and the saved CLI logins."
		if [ "$VOLUMES" = 1 ]; then
			plan="$plan
${C_ERR}It deletes every app volume: the data in them is gone for good.${C_RESET}"
		else
			plan="$plan
App volumes are kept (add --volumes to delete them and their data)."
		fi
	else
		plan="$plan
Your apps keep running and $DATA_DIR is kept (add --purge to remove everything)."
	fi
	confirm "$plan"
	[ "$PURGE" = 1 ] && clean_remote_servers

	step "Removing $DAEMON_BIN"
	if has_systemd && [ -f "/etc/systemd/system/$UNIT" ]; then
		systemctl disable --now "$UNIT" >/dev/null 2>&1 || true
		rm -rf "/etc/systemd/system/$UNIT" "/etc/systemd/system/$UNIT.d"
		systemctl daemon-reload
		removed "the $UNIT service"
	elif pkill -x "$DAEMON_BIN" 2>/dev/null; then
		removed "the running $DAEMON_BIN process"
	fi
	for bin in "$DAEMON_BIN" "$CLI_BIN"; do
		if [ -e "$BIN_DIR/$bin" ]; then
			rm -f "$BIN_DIR/$bin"
			removed "$BIN_DIR/$bin"
		fi
	done

	if [ "$PURGE" = 1 ]; then
		purge_local
	fi

	print_removed
	if [ "$PURGE" = 1 ]; then
		[ "$VOLUMES" = 1 ] || note "app volumes were kept: docker volume ls --filter label=$PREFIX.managed=true"
		ok "$NAME is gone from this server"
	else
		ok "Removed $NAME; your apps keep running and $DATA_DIR was kept"
	fi
}

count() { echo "$1" | sed '/^$/d' | wc -l | tr -d ' '; }

purge_local() {
	if has docker && docker info >/dev/null 2>&1; then
		ids=$(docker ps -aq --filter "label=$PREFIX.managed=true")
		if [ -n "$ids" ]; then
			# shellcheck disable=SC2086
			docker rm -f $ids >/dev/null
			removed "$(count "$ids") app containers"
		fi
		if docker rm -f "$PREFIX-proxy" >/dev/null 2>&1; then
			removed "the $PREFIX-proxy container"
		fi
		if docker rm -f "$PREFIX-postgres" >/dev/null 2>&1; then
			removed "the $PREFIX-postgres store container"
		fi
		if docker volume rm "$PREFIX-pgdata" >/dev/null 2>&1; then
			removed "the $PREFIX-pgdata store volume"
		fi
		nets=$(docker network ls -q --filter "name=^$PREFIX")
		if [ -n "$nets" ]; then
			echo "$nets" | xargs docker network rm >/dev/null 2>&1 || true
			removed "$(count "$nets") networks"
		fi
		if [ "$VOLUMES" = 1 ]; then
			vols=$(docker volume ls -q --filter "label=$PREFIX.managed=true")
			if docker volume inspect "$PREFIX-caddy-data" >/dev/null 2>&1; then
				vols="$vols
$PREFIX-caddy-data"
			fi
			vols=$(echo "$vols" | sed '/^$/d')
			if [ -n "$vols" ]; then
				echo "$vols" | xargs docker volume rm >/dev/null
				removed "$(count "$vols") volumes and the data in them"
			fi
		fi
	fi
	if [ -d "$DATA_DIR" ]; then
		rm -rf "$DATA_DIR"
		removed "$DATA_DIR"
	fi
	[ "$KEEP_CONFIG" = 0 ] || return 0
	for home in /root $(sudo_home); do
		if [ -d "$home/.config/$DIR_NAME" ]; then
			rm -rf "$home/.config/$DIR_NAME"
			removed "$home/.config/$DIR_NAME"
		fi
	done
}

# Removes the CLI from wherever 'install.sh --cli' put it for this user.
uninstall_cli() {
	if [ "$(id -u)" = 0 ]; then
		dir="$BIN_DIR"
	else
		dir="$HOME/.local/bin"
	fi
	config_dir="${XDG_CONFIG_HOME:-$HOME/.config}/$DIR_NAME"
	plan="This removes $dir/$CLI_BIN"
	[ "$KEEP_CONFIG" = 0 ] && plan="$plan and your saved logins in $config_dir"
	confirm "$plan."
	if [ -e "$dir/$CLI_BIN" ]; then
		rm -f "$dir/$CLI_BIN"
		removed "$dir/$CLI_BIN"
	fi
	if [ "$KEEP_CONFIG" = 0 ] && [ -d "$config_dir" ]; then
		rm -rf "$config_dir"
		removed "$config_dir"
		note "tokens saved there keep working until you run '$CLI_BIN token rotate' against each daemon"
	fi
	print_removed
}

case "$UNINSTALL:$MODE" in
0:server) install_server ;;
0:cli) install_cli ;;
1:server) uninstall_server ;;
1:cli) uninstall_cli ;;
esac
