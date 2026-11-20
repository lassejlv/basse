#!/bin/sh
# Removes Basse from this machine.
#
#   On the control server, keeping app data:
#     curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/uninstall.sh | sudo sh
#   Everything, remote servers included (add --volumes to delete app data too):
#     curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/uninstall.sh | sudo sh -s -- --purge
#   Only the CLI, e.g. on a laptop:
#     curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/uninstall.sh | sh -s -- --cli
#
# Options: --purge, --volumes, --cli, --keep-config, -y/--yes. It asks before removing anything
# when a terminal is attached.
#
# The uninstall logic lives in install.sh (--uninstall) so there is one copy of it. This script
# runs the install.sh next to it, or downloads it when piped from curl.

set -eu

# Product names. Keep in sync with crates/common/src/brand.rs.
NAME="Basse"
REPO="lassejlv/basse"

for arg in "$@"; do
	case "$arg" in
	-h | --help)
		cat <<EOF
$NAME uninstaller

Usage: uninstall.sh [--purge [--volumes]] [--cli] [--keep-config] [--yes]

  (no options)     Remove the daemon, its service and the binaries; keep data and apps
  --purge          Also clean up every remote server, then remove app containers, the proxy,
                   networks, the data directory and the CLI logins on this server
  --volumes        With --purge: also delete the app volumes. This deletes app data.
  --cli            Remove only the CLI and its saved logins, e.g. on a laptop
  --keep-config    Keep the CLI's saved logins
  -y, --yes        Don't ask for confirmation
EOF
		exit 0
		;;
	esac
done

case "$0" in
*uninstall.sh)
	here=$(dirname "$0")
	if [ -f "$here/install.sh" ]; then
		exec sh "$here/install.sh" --uninstall "$@"
	fi
	;;
esac

tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT INT TERM
curl -fsSL "https://raw.githubusercontent.com/$REPO/main/install.sh" -o "$tmp" || {
	echo "error: cannot download the $NAME installer" >&2
	exit 1
}
sh "$tmp" --uninstall "$@"
