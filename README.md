# Berth

Self-hosted deploys from the terminal. A control daemon runs on your server and builds, runs and routes your apps. You drive it with a CLI and a full-screen TUI, from that machine or from your laptop.

- **Projects and environments.** Group services into projects, each with `production`, `staging` or any other environment. Services in one environment reach each other by name.
- **Three kinds of source.** Build a git repository with its Dockerfile, run a registry image, or start a one-click database (Postgres, MySQL, MariaDB, Redis, Mongo) with generated credentials, a data volume and a `DATABASE_URL`.
- **Zero-downtime deploys.** A new container must pass its health check before the proxy sends it traffic. Then the old one stops. Rollback redeploys an earlier image without rebuilding.
- **Domains with automatic HTTPS.** Caddy runs as the reverse proxy on each server and gets certificates on its own.
- **Logs, env vars and volumes.** Stream container and build logs, manage variables and secrets, and mount named volumes that survive redeploys.
- **More servers.** Register extra machines over SSH, and the daemon deploys to them through the same API.

## Install

On the server (needs Docker and git):

```sh
cargo install --git https://github.com/lassejlv/rust-deploy daemon
sudo berthd init            # prints the API token and the login command
sudo berthd install-service # systemd unit; or run `berthd serve`
```

On your machine:

```sh
cargo install --git https://github.com/lassejlv/rust-deploy cli
berth login http://<server-ip>:7700 --token <token>
berth                        # opens the TUI
```

## TUI

Run `berth` with no arguments. Press `?` for every key and `:` for the command palette.

| Screen | Keys |
|---|---|
| Overview | `↑↓` move, `tab` switch pane, `⏎` open, `n` new service, `N` new project, `E` new environment, `d` deploy, `r` restart, `s` stop or start, `l` logs, `/` filter, `S` servers |
| Service | `1`–`7` or `tab` switch tabs (Overview, Deployments, Logs, Env, Domains, Volumes, Settings), `a` add, `x` remove, `⏎` edit |
| Logs | `f` follow, `/` search, `n`/`N` next or previous match, `e` errors only, `t` timestamps, `w` wrap |
| Deploy | Live pipeline and build output, `c` cancel, `esc` back (the deploy keeps running) |

## CLI

```sh
berth project create acme-shop
berth service create acme-shop web --repo https://github.com/acme/web --port 3000 --domain shop.example.com
berth service create acme-shop --database postgres
berth vars set web STRIPE_KEY=sk_live_… --secret --deploy
berth logs web -f
berth deploy web
berth rollback web
berth status
```

Every command takes `--json` and `--context <name>` (switch between daemons with `berth context use`). Run `berth <command> --help` for the details.

## Workspace

| Crate | What it is |
|---|---|
| `crates/common` | Domain types, the HTTP API contract and `brand`, the one place that holds product names |
| `crates/client` | Typed HTTP client and the saved daemon contexts |
| `crates/daemon` | The `berthd` binary: axum API, SQLite, Docker through bollard, Caddy proxy, deploy pipeline, SSH servers |
| `crates/tui` | The ratatui interface |
| `crates/cli` | The `berth` binary: clap commands that open the TUI by default |

```sh
cargo test --workspace
cargo test -p daemon --test docker_e2e -- --ignored --test-threads 1   # needs Docker and free ports 80/443
```

The TUI design lives in Paper: [Berth TUI](https://app.paper.design/file/01M7X3Z4P0WETSB6Q8Q6WXA4TR/p-1-0).
