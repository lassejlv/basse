# Basse

Self-hosted deploys from the terminal. A control daemon runs on your server and builds, runs and routes your apps. You drive it with a CLI and a full-screen TUI, from that machine or from your laptop.

- **Projects and environments.** Group services into projects, each with `production`, `staging` or any other environment. Services in one environment reach each other by name.
- **Three kinds of source.** Build a git repository with its Dockerfile, run a registry image, or start a one-click database (Postgres, MySQL, MariaDB, Redis, Mongo) with generated credentials, a data volume and a `DATABASE_URL`.
- **Zero-downtime deploys.** A new container must pass its health check before the proxy sends it traffic. Then the old one stops. Rollback redeploys an earlier image without rebuilding.
- **Domains with automatic HTTPS.** Caddy runs as the reverse proxy on each server and gets certificates on its own.
- **Logs, env vars and volumes.** Stream container and build logs, manage variables and secrets, and mount named volumes that survive redeploys.
- **Database backups and access.** Back up one-click databases on demand or daily with retention, download and restore dumps, rotate passwords in place, and publish a database on a server port when you need to reach it from outside.
- **More servers.** Register extra machines over SSH, and the daemon deploys to them through the same API.

## Install

On a fresh Linux server (Ubuntu, Debian, Fedora, RHEL, Alpine, Arch; x86_64 or arm64), run:

```sh
curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/install.sh | sudo sh
```

The script needs no other preparation:

1. It installs curl, git and ssh, then Docker if it's missing (via get.docker.com), and makes sure Docker is running.
2. It downloads the static `bassed` and `basse` binaries from the latest GitHub release and checks their SHA-256. If there is no release for your platform, it builds them from source.
3. It initializes `/var/lib/basse`, installs a systemd unit and starts the daemon.
4. The daemon starts the Caddy reverse proxy on ports 80 and 443 by itself.
5. It opens ports 80, 443 and 7700 in ufw or firewalld if one of them is active.
6. It logs in the `basse` CLI for root and for the user who ran sudo.
7. It prints the API URL, the token and the login command for your laptop.

Running the script again upgrades in place and keeps your data and token. Other options:

```sh
… | sudo sh -s -- --version v0.2.0     # pin a release
… | sudo sh -s -- --from-source        # build with cargo instead of downloading
```

On your laptop (macOS or Linux), install only the CLI:

```sh
curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/install.sh | sh -s -- --cli
basse login http://<server-ip>:7700 --token <token>
basse
```

## Update

```sh
basse update     # update the daemon of the current context and wait for it to restart
basse upgrade    # update this CLI binary (use sudo if it lives in /usr/local/bin)
```

The daemon downloads the release for its platform, checks its SHA-256, replaces its own binary and the `basse` CLI next to it, and exits so systemd starts the new version. It refuses while a deployment is running. The TUI shows "update available" on the Settings screen, where `u` does the same.

## Uninstall

```sh
curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/uninstall.sh | sudo sh
```

That removes the daemon, its systemd unit and the binaries, and keeps your apps running and `/var/lib/basse` in place. It asks before removing anything when a terminal is attached; `--yes` skips the question.

- `--purge` removes everything: it first asks the daemon to clean up each remote SSH server (containers, proxy and networks; a server it cannot reach is skipped with the command to clean it up by hand), then removes the app containers, the proxy, the networks, `/var/lib/basse` and the saved CLI logins on this server. App volumes are kept.
- `--purge --volumes` also deletes the app volumes. **This deletes your app data.**
- `--cli` removes only the CLI and its saved logins, e.g. on your laptop: `curl … | sh -s -- --cli`. Add `--keep-config` to keep the logins.

`install.sh --uninstall` takes the same options.

While the repository is private, raw.githubusercontent.com and release downloads need a token. Pass one with `GITHUB_TOKEN=… sh install.sh` or `--github-token`, and fetch the script itself with `curl -H "Authorization: Bearer $GITHUB_TOKEN" …`.

## Releases

Pushing a `v*` tag runs `.github/workflows/release.yml`. It builds static musl binaries for x86_64 and arm64 Linux (daemon and CLI) plus the macOS CLI. It then installs a server from the fresh archive on a clean runner as a smoke test, and publishes the archives and checksums that `install.sh` downloads.

## TUI

Run `basse` with no arguments. Press `?` for every key and `:` for the command palette.

The mouse works too: click rows, tabs, dialog fields and the key hints in the bottom bar, double-click a service to open it, and use the wheel to scroll lists and logs. Drag to select text anywhere; it is copied to your clipboard on release through OSC 52, which also works over SSH. Hold Shift while dragging to use your terminal's own selection instead.

| Screen | Keys |
|---|---|
| Overview | `↑↓` move, `tab` switch pane, `⏎` open, `n` new service, `N` new project, `E` new environment, `d` deploy, `r` restart, `s` stop or start, `l` logs, `/` filter, `S` servers |
| Service | `1`–`7` or `tab` switch tabs (Overview, Deployments, Logs, Env, Domains, Volumes, Settings), `a` add, `x` remove, `⏎` edit |
| Logs | `f` follow, `/` search, `n`/`N` next or previous match, `e` errors only, `t` timestamps, `w` wrap |
| Deploy | Live pipeline and build output, `c` cancel, `esc` back (the deploy keeps running) |
| Servers | `a` add, `c` check, `p` set up the proxy, `x` remove, `X` remove and clean up everything the daemon put there |
| Settings (`,` anywhere) | The instance (version and updates, public IP, data dir, Docker), the proxy, the API token and SSH key, pruning old images, and the daemon contexts saved on this machine. `u` update, `i` public IP, `m` auto-manage the proxy, `r` restart the proxy, `t` rotate the token, `k` copy the SSH key, `c` prune, `⏎` switch to the selected context, `x` forget it |

## CLI

```sh
basse project create acme-shop
basse service create acme-shop web --repo https://github.com/acme/web --port 3000 --domain shop.example.com
basse service create acme-shop --database postgres
basse vars set web STRIPE_KEY=sk_live_… --secret --deploy
basse db info postgres --reveal
basse db backup postgres
basse db schedule postgres --hour 3 --keep 7
basse logs web -f
basse deploy web
basse rollback web
basse status
```

The instance itself:

```sh
basse settings                          # version, public IP, proxy, disk use, reclaimable space
basse settings set public-ip 203.0.113.10   # or "auto" to detect it again
basse settings set manage-proxy off
basse token rotate                      # the old token stops working; the new one is saved for you
basse proxy restart
basse prune                             # old build images (live and previous kept) and the build cache
basse server remove edge --purge [--volumes]
basse logout [context] [--all]          # forgets the token here; rotate it to revoke it
```

Every command takes `--json` and `--context <name>` (switch between daemons with `basse context use`). Run `basse <command> --help` for the details.

## Workspace

| Crate | What it is |
|---|---|
| `crates/common` | Domain types, the HTTP API contract and `brand`, the one place that holds product names |
| `crates/client` | Typed HTTP client and the saved daemon contexts |
| `crates/release` | Release lookups, verified downloads and binary replacement for `update` and `upgrade` |
| `crates/daemon` | The `bassed` binary: axum API, SQLite, Docker through bollard, Caddy proxy, deploy pipeline, SSH servers |
| `crates/tui` | The ratatui interface |
| `crates/cli` | The `basse` binary: clap commands that open the TUI by default |

```sh
cargo test --workspace
cargo test -p daemon --test docker_e2e -- --ignored --test-threads 1   # needs Docker and free ports 80/443
```

The TUI design lives in Paper: [Basse TUI](https://app.paper.design/file/01M7X3Z4P0WETSB6Q8Q6WXA4TR/p-1-0).
