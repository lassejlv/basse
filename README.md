# Basse

Self-hosted deploys from the terminal. A control daemon runs on your server and builds, runs and routes your apps. You drive it with a CLI and a full-screen TUI, from that machine or from your laptop.

- **Projects and environments.** Group services into projects, each with `production`, `staging` or any other environment. Services in one environment reach each other by name.
- **Three kinds of source.** Build a git repository with its Dockerfile, run a registry image, or start a one-click database (Postgres, MySQL, MariaDB, Redis, Mongo) with generated credentials, a data volume and a `DATABASE_URL`.
- **Zero-downtime deploys.** A new container must pass its health check before the proxy sends it traffic. Then the old one stops. Rollback redeploys an earlier image without rebuilding.
- **Domains with automatic HTTPS.** Caddy runs as the reverse proxy on each server and gets certificates on its own.
- **Logs, env vars and volumes.** Stream container and build logs, manage variables and secrets, and mount named volumes that survive redeploys.
- **Database backups and access.** Back up one-click databases on demand or daily with retention, download and restore dumps, rotate passwords in place, and publish a database on a server port when you need to reach it from outside.
- **More servers.** Register extra machines over SSH, with the daemon's key or an SSH key from the secrets manager, and the daemon deploys to them through the same API.
- **Secrets manager.** SSH keys, S3 credentials and other values, encrypted at rest with a master key. Values are shown only when you explicitly reveal them, and every reveal is logged.
- **S3 storage.** Keep database backups in AWS S3, Cloudflare R2, Backblaze B2, MinIO or any other S3-compatible bucket.
- **Instance backups.** One encrypted archive of the daemon itself, and `bassed restore` to bring it back on a new machine.

## Install

On a fresh Linux server (Ubuntu, Debian, Fedora, RHEL, Alpine, Arch; x86_64 or arm64), run:

```sh
curl -fsSL https://raw.githubusercontent.com/lassejlv/basse/main/install.sh | sudo sh
```

The script needs no other preparation:

1. It installs curl, git and ssh, then Docker if it's missing (via get.docker.com), and makes sure Docker is running.
2. It downloads the static `bassed` and `basse` binaries from the latest GitHub release and checks their SHA-256. If there is no release for your platform, it builds them from source.
3. It initializes `/var/lib/basse`, installs a systemd unit and starts the daemon. The daemon keeps its state in a Postgres 18 container it runs itself (`basse-postgres`, published only on `127.0.0.1:5439`, data in the `basse-pgdata` volume, password generated into `config.toml`).
4. The daemon starts the Caddy reverse proxy on ports 80 and 443 by itself.
5. It opens ports 80, 443 and 7700 in ufw or firewalld if one of them is active.
6. It logs in the `basse` CLI for root and for the user who ran sudo.
7. It prints the API URL, the token and the login command for your laptop.

Running the script again upgrades in place and keeps your data and token. Installs from before the Postgres store kept their state in `/var/lib/basse/basse.db` (SQLite); the upgraded daemon imports that file on its first start, checks the row counts, and renames it to `basse.db.migrated-<timestamp>.bak`. If the import fails it leaves the file untouched and refuses to start, saying why; `bassed migrate-from-sqlite <file>` runs the same import by hand.

To use your own Postgres instead of the managed container, initialize with `bassed init --database-url postgres://…`, set `database_url` in `config.toml`, or set `BASSE_DATABASE_URL`. `--postgres-port` (or `port` under `[postgres]` in `config.toml`) moves the managed container to another local port.

Other options:

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
| Service | `1`–`9` or `tab` switch tabs (Overview, Deployments, Logs, Database, Backups, Env, Domains, Volumes, Settings; Database and Backups only for databases), `a` add, `x` remove, `⏎` edit |
| Database | `v` reveal the password, `c`/`C` copy the internal or public URL, `p` expose on a server port or make internal again, `R` rotate the password |
| Backups | `b` back up now (asks where when storage connections exist), `R` restore, `D` download to a local file, `x` delete, `e` edit the daily schedule and its store |
| Logs | `f` follow, `/` search, `n`/`N` next or previous match, `e` errors only, `t` timestamps, `w` wrap |
| Deploy | Live pipeline and build output, `c` cancel, `esc` back (the deploy keeps running) |
| Servers | `a` add (pick an SSH key or generate one), `K` change a server's key, `c` check, `p` set up the proxy, `x` remove, `X` remove and clean up everything the daemon put there |
| Secrets (`s` in Settings, or the palette) | `a` add or import, `g` generate an SSH key, `v` reveal (logged), `c` copy the public key, `n` rename, `e` replace the value, `x` delete |
| Storage (`o` in Settings) | `a` add, `⏎` edit, `t` test, `x` remove |
| Notifications (`n` in Settings, or the palette) | `tab` switch between Channels, Rules and Delivery log. Channels: `a` add (kind, fields, test, save), `⏎` edit, `t` test, `e` enable or disable, `x` delete. Rules: `a` add (channel, scope, dedupe and rate limit, a preset, then toggle events with `space`), `⏎` edit, `e` enable or disable, `x` delete. Delivery log: `r` refresh, `f` failures only, `c` this channel only |
| Settings (`,` anywhere) | The instance (version and updates, public IP, data dir, Docker), the proxy, the API token and SSH key, pruning old images, and the daemon contexts saved on this machine. `u` update, `i` public IP, `m` auto-manage the proxy, `r` restart the proxy, `t` rotate the token, `k` copy the SSH key, `c` prune, `⏎` switch to the selected context, `x` forget it. With `tab` on Instance backups: `b` back up now, `e` schedule, `D` download, `x` delete |

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

## Instance domain

Give the daemon a domain and the proxy serves its API there with automatic HTTPS, next to your services. GitHub Apps need it, because GitHub calls back to the instance.

```sh
basse instance domain set deploy.example.com   # routes the domain to the API, prints the DNS records, runs the checks
basse instance domain check                    # run the checks again after changing DNS
basse instance domain                          # show the records and the last check
basse instance domain rm                       # back to IP:port only
```

Create the records it prints (an `A` record for the public IPv4 address, `AAAA` for IPv6) at your DNS provider, with any CDN proxying turned off. The checks then confirm, in order, that the domain resolves to this server, that ports 80 and 443 accept connections, that a certificate was issued, and that the API answers over HTTPS; each failing step says what to change. Once everything passes, `basse login` and the current context switch to `https://<domain>`. `http://<ip>:7700` keeps working. In the TUI the same guide is Settings → Domain (`d`).

## Git sources

A git source lets services clone private repositories and deploy on push. Basse creates a GitHub App for you with GitHub's manifest flow:

```sh
basse source github create acme --org acme       # opens GitHub to create the app in the acme organization, then to install it
basse source github create mine                  # same, on your personal account
basse source github create corp --github-url https://github.corp.example   # GitHub Enterprise
basse source github connect old --app-id 12345 --private-key-file app.pem --webhook-secret-file hook.txt
basse source github token small --token-file pat.txt   # a personal access token; no push deploys
basse source list
basse source repos acme --search api             # repositories the installation can see
basse source branches acme acme/api
basse source check acme
basse source rm acme                             # refused while a service clones from it

basse service create shop/production --source acme --repo acme/api --branch main --port 8080
```

You can have as many sources as you like, for example your personal account plus two organizations. The app's private key, webhook secret and client credentials are stored as a `github_app` secret, encrypted like every other secret, and are never returned by the API. Clones use an installation token that lives for an hour and reaches git through its environment, never a file or the command line. A push to a service's branch deploys it (turn that off with `--no-auto-deploy`); webhook deliveries are checked against the app's webhook secret. Preview deployments for pull requests are not built yet.

### Build methods

Git services build one of three ways, picked with `--build` (default `auto`) and changed later with `basse service build <service> --method …`; a change applies on the next deploy.

| Method | What happens | Options |
|---|---|---|
| `dockerfile` | `docker build` of the Dockerfile; the service's env vars are passed as build args | `--dockerfile`, `--build-context`, `--target <stage>` |
| `compose` | `docker compose up` of the compose file as one group: every compose service gets its own status and logs (`basse logs <service> --container web`), named volumes survive redeploys, and the service's env vars are available for `${VAR}` interpolation | `--compose-file`; domains take `--compose-service web` |
| `railpack` | [Railpack](https://railpack.com) detects the language and builds with BuildKit, no Dockerfile needed; the daemon downloads Railpack the first time and checks its checksum | |
| `auto` | a Dockerfile wins, then a compose file (`compose.yaml`, `docker-compose.yml`, …), else Railpack; the result is saved on the service at the first deploy | |

```sh
basse service create shop/production --source acme --repo acme/api --build compose
basse domain add stack shop.example.com --compose-service web
basse service build api --method dockerfile --target prod
```

In a monorepo, point each service at its app with `--root`: the Dockerfile, build context and compose file are then relative to that directory, compose runs with it as the project directory, Railpack builds it, and `auto` looks there. A push only deploys a service with a root when it changed a file inside the root; pass `--no-watch-paths` (or `basse service build <service> --watch-paths false`) to deploy on every push.

```sh
basse service create shop/production api --source acme --repo acme/mono --root apps/api
basse service create shop/production web --source acme --repo acme/mono --root apps/web
basse service build web --root apps/site
```

Without a source, `--repo` still takes any clone URL: public repositories clone anonymously, and `git@github.com:` URLs use the daemon's SSH key, which you can add to a repository as a deploy key (`basse server key` prints it).

## Secrets

Secrets hold values the daemon uses but nobody should read in passing: SSH private keys for servers, S3 credentials for storage, and generic values.

```sh
basse secrets generate-ssh-key edge-key                   # ed25519, generated in the daemon; prints the public key
basse secrets add deploy-key --kind ssh-key --file ~/.ssh/id_ed25519
basse secrets add r2 --kind s3 --access-key-id <id> --file secret.txt   # the secret key comes from the file or stdin
basse secrets add stripe --kind generic < value.txt
basse secrets list                                        # kind, public part, what uses it; never values
basse secrets show edge-key                               # includes the public key to put in authorized_keys
basse secrets reveal stripe                               # prints the value and records it in the activity feed
basse secrets rename stripe stripe-live
basse secrets replace r2 --access-key-id <id> --file new-secret.txt
basse secrets rm stripe                                   # refused while a server or storage uses it
```

Values are never taken as command-line arguments, so they stay out of shell history and process lists. The API returns a value only from `POST /secrets/{id}/reveal`; lists, `--json` output, activity rows and error messages never contain one.

**Encryption.** `bassed init` (or the first start of an existing install) writes 32 random bytes to `/var/lib/basse/master.key` with mode 0600. Each value is encrypted with XChaCha20-Poly1305 under that key with a fresh random nonce, and the secret's id is authenticated as associated data, so a ciphertext copied onto another row does not decrypt. The database only ever sees ciphertext. Losing `master.key` means losing every secret, so keep instance backups (below), which include it.

**SSH keys for servers.** `basse server add edge 203.0.113.7 --key edge-key` connects with that key instead of the daemon's own one, and `basse server set-key edge [<secret>]` switches a server to another key (no secret goes back to the daemon's key). Put the key's public part (`basse secrets show edge-key`) in `~/.ssh/authorized_keys` on the server. The daemon hands the key to `ssh` as a mode-0600 file in `/var/lib/basse/ssh/keys` that exists only during the handshake.

## Storage

A storage connection is an S3-compatible bucket for backups. Its credentials are an `s3_credentials` secret.

```sh
basse secrets add r2 --kind s3 --access-key-id <id> --file secret.txt
basse storage add offsite --endpoint https://<account>.r2.cloudflarestorage.com --region auto \
  --bucket backups --prefix basse --credentials r2      # adds it and runs a test
basse storage test offsite                               # put, read back and delete a probe object, with latency
basse storage list
basse storage rm offsite                                 # refused while backups or schedules use it
```

| Provider | `--endpoint` | `--region` | Notes |
|---|---|---|---|
| AWS S3 | `https://s3.<region>.amazonaws.com` | the bucket's region | |
| Cloudflare R2 | `https://<account id>.r2.cloudflarestorage.com` | `auto` | |
| Backblaze B2 | `https://s3.<region>.backblazeb2.com` | e.g. `us-west-004` | |
| MinIO, Garage, Ceph | your server's URL | as configured | add `--path-style` |

Database backups then take a target:

```sh
basse db backup postgres --storage offsite
basse db schedule postgres --hour 3 --keep 7 --storage offsite   # --storage local goes back to the data dir
basse db backups postgres                                         # the WHERE column shows local or s3:<name>
```

Objects are stored as `<prefix>/<service id>/<file>`. Uploads stream from disk in 8 MiB multipart chunks and downloads stream as they arrive, so a backup is never held in memory whole. Download and restore work the same whichever store a backup is in. Retention runs per store, so moving a schedule to S3 never prunes the backups still kept locally.

## Instance backups and disaster recovery

An instance backup is one archive of the daemon itself: a `pg_dump -Fc` of its store, `config.toml`, `master.key`, the `ssh/` keys and known hosts, and a manifest of the local database backup files (the files themselves are not included; back them up to S3 storage to keep them off the machine). The archive contains the master key, so it is always encrypted with a passphrase you choose. The passphrase is never stored for manual backups; without it the archive cannot be opened, by you or anyone else.

```sh
basse instance backup --storage offsite --passphrase-file pass.txt   # or pipe the passphrase on stdin
basse instance backups
basse instance download [<backup>] [-o file]
basse instance rm <backup>
basse instance schedule --hour 2 --keep 7 --storage offsite --passphrase-file pass.txt
basse instance schedule --off
```

The archive is a `tar.gz` encrypted with XChaCha20-Poly1305 in the STREAM construction (64 KiB chunks), under a key derived from the passphrase with argon2id (64 MiB, 3 passes). Any change to the file, including cutting it short, makes decryption fail.

**The schedule stores the passphrase.** Scheduled runs need the passphrase without you, so `instance schedule` keeps it encrypted under the master key. That is a trade-off: anyone who gets the data directory (and with it `master.key` and the store) can recover the passphrase and open every archive made with it. They already hold everything those archives contain, so the archives in S3 stay useless to someone who has only the bucket. If that is not acceptable, run manual backups from your laptop instead (`basse instance backup` from a cron job there), where the passphrase never reaches the server.

### Disaster recovery runbook

The old server is gone and you have an instance backup in S3 (or a downloaded archive) and its passphrase.

1. **Install a fresh server** with the one-line install above. It creates a new, empty daemon with its own managed Postgres.
2. **Put the passphrase in a file** that only root can read:
   ```sh
   install -m 600 /dev/null /root/pass.txt && printf '%s' '<passphrase>' > /root/pass.txt
   ```
3. **Restore.** From S3, with credentials that can read the bucket (from a local file, drop the `--s3-*` flags and the environment variables):
   ```sh
   sudo AWS_ACCESS_KEY_ID=<id> AWS_SECRET_ACCESS_KEY=<secret> bassed restore \
     s3://backups/basse/instance/instance-20261120-020000-14.tar.gz.enc \
     --passphrase-file /root/pass.txt \
     --s3-endpoint https://<account>.r2.cloudflarestorage.com --s3-region auto
   ```
   Use `--s3-path-style` for MinIO and other self-hosted stores. `bassed restore` stops the `bassed` service, puts back `config.toml`, `master.key` and `ssh/`, replaces the store with the dump (in the managed container, or in the external database the old config names; `--database-url` picks another one), checks that every secret decrypts, starts the service again and checks every server.
4. **Point your CLI at the new server.** The API token is the old one, so `basse login http://<new-ip>:7700 --token <old token>` works, or edit the context's URL in `~/.config/basse/config.toml`. Run `basse status`.
5. **Remote servers** trust the same keys as before, because the SSH keys came back with the archive. Servers that show offline in the restore output usually only need a firewall rule for the new IP.
6. **Redeploy apps that ran on the old machine itself.** The store knows every service, but their containers and volumes were on the lost server. Run `basse deploy <service>` for them, and restore databases from their S3 backups with `basse db restore <service> <backup>`. Local database backups were on the old disk; the restore prints how many are missing.
7. **Point DNS** for your domains at the new server so the proxy can get certificates again.
8. **Take a fresh instance backup** once everything is green.

## Notifications

A channel is somewhere messages go; a rule sends a set of events from the whole instance, one project, one environment or one service to a channel. Nothing is sent until a rule says so.

```sh
basse notify channel add ops-discord --kind discord < webhook-url.txt      # the credential comes from stdin or --secret-file
basse notify channel add oncall --kind smtp --host smtp.example.com --username alerts \
  --from "Alerts <alerts@example.com>" --to oncall@example.com --secret-file smtp-password.txt
basse notify channel add phone --kind ntfy --topic acme-alerts            # server defaults to https://ntfy.sh
basse notify channel list                                                   # targets are shown redacted
basse notify channel test ops-discord

basse notify presets                                                        # essentials, deploys, databases, everything
basse notify rule add --channel ops-discord --preset essentials
basse notify rule add --channel ops-discord --preset deploys --project acme --exclude deploy.queued
basse notify rule add --channel phone --event service.crashed --event "server.*" --service acme/production/api
basse notify rule list
basse notify log                                                            # the last attempts with status and error
basse notify events                                                         # the whole catalog
```

| Kind | Needs | Credential (stdin or `--secret-file`) |
|---|---|---|
| `smtp` | `--host`, `--port`, `--security` (starttls, tls, none), `--username`, `--from`, `--to` | password (with `--username`) |
| `resend` | `--from`, `--to` | API key |
| `cloudflare` | `--account-id`, `--from`, `--to` (Cloudflare Email Service, sending domain onboarded) | API token with email sending permission |
| `discord`, `slack`, `mattermost`, `google-chat` | | webhook URL |
| `teams` | a Workflows "post to a channel when a webhook request is received" flow | webhook URL |
| `telegram` | `--chat-id` | bot token |
| `ntfy` | `--topic`, `--server` | access token, optional (`--secret-file`) |
| `gotify` | `--server` | app token |
| `pushover` | `--user` | app token |
| `webhook` | `--url`, `--header "Name: value"` | HMAC secret, optional (`--secret-file`) |

**Events.** Deploys (queued, started, succeeded, failed, cancelled, rolled back, health check failed), services (started, stopped, restarted, crashed, killed out of memory, restart loop), databases (created, backup succeeded or failed, restore done or failed, password rotated), instance backups, S3 storage unreachable, TLS certificates (issued or renewed, expiring soon, invalid), servers (connected, unreachable, disk or memory above 90% on the daemon's machine), the daemon (started, update available, update installed) and security (API token rotated, secret changed, secret revealed). Each has a stable id such as `deploy.failed` and a severity that colors the message.

**Presets** fill a rule's events in one pick and can be adjusted afterwards: *Essentials* (every critical event: failures and outages), *Deploys* (the whole deploy lifecycle), *Databases & backups* (databases, instance backups and storage) and *Everything*. A rule without a preset is a custom pick.

**Delivery.** Events are queued and sent in the background, so a slow channel never holds up a deploy. A failed attempt is retried after 5 s, 30 s and 2 min when retrying can help (timeouts, 5xx, 429). Each rule drops the same event about the same subject within its dedupe window (5 minutes by default) and sends at most 30 messages an hour; dropped ones show as `suppressed` in the log, which keeps the last 1000 attempts. Generic webhooks get a JSON body with the event id, severity, subject ids and fields, and an `X-Basse-Signature: sha256=<hex>` HMAC of the body when a secret is set.

**Credentials.** A channel's whole config is encrypted with the master key like a secret, bound to the channel's id. The API only ever returns it redacted: passwords, tokens and header values become `[redacted]` and webhook URLs are cut to their host.

## Workspace

| Crate | What it is |
|---|---|
| `crates/common` | Domain types, the HTTP API contract and `brand`, the one place that holds product names |
| `crates/client` | Typed HTTP client and the saved daemon contexts |
| `crates/release` | Release lookups, verified downloads and binary replacement for `update` and `upgrade` |
| `crates/daemon` | The `bassed` binary: axum API, Postgres through sqlx, Docker through bollard, Caddy proxy, deploy pipeline, SSH servers |
| `crates/tui` | The ratatui interface |
| `crates/cli` | The `basse` binary: clap commands that open the TUI by default |

```sh
cargo test --workspace   # starts throwaway postgres:18-alpine and MinIO containers; skips without Docker
cargo test -p daemon --test docker_e2e -- --ignored --test-threads 1   # needs Docker and free ports 80/443
```

The TUI design lives in Paper: [Basse TUI](https://app.paper.design/file/01M7X3Z4P0WETSB6Q8Q6WXA4TR/p-1-0).
