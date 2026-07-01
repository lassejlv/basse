# Basse agent

The on-server agent. It runs as a container on each user server, mounts the
Docker socket, and exposes a small bearer-authenticated HTTP API.

Basse can reach the agent in two ways:

- **SSH mode**: the default. The control plane opens an SSH tunnel to the
  server and calls the agent on loopback.
- **Outbound mode**: for servers with no public ingress and no SSH access from
  Basse. The agent polls the control plane over HTTPS, pulls queued commands,
  runs them against its local HTTP API, then posts the result back.

## Why Go (and no dependencies)

The agent is a single static binary with **zero external dependencies** — it
talks to the Docker Engine API directly over the mounted unix socket using only
the standard library (`internal/dockerx`). That keeps the image tiny and the
build hermetic (no `go.sum`, no module downloads).

## Transport / security

- In SSH mode, the container publishes its port to **host loopback only**
  (`docker run -p 127.0.0.1:8888:8888`). There is no public port.
- The control plane reaches SSH-mode agents over an on-demand `ssh -L` tunnel reusing
  the per-server provisioning key, so the bearer token never crosses the network.
- In outbound mode, the agent does not publish a host port. It makes outbound
  HTTPS requests to the control plane endpoint configured by
  `BASSE_CONTROL_PLANE_URL`.
- `/v1/*` routes require `Authorization: Bearer <BASSE_AGENT_TOKEN>`, compared in
  constant time. The agent refuses to start if the token is empty.
- Outbound polling also uses `Authorization: Bearer <BASSE_AGENT_TOKEN>`. The
  API stores an encrypted token plus a SHA-256 lookup hash; the raw token is
  shown only in the one-time install command.
- It runs as **root** so it can read `/var/run/docker.sock` (owned `root:docker`).

## Connection modes

### SSH mode

SSH mode is the normal self-hosted flow:

1. A user adds a server with SSH host, user, port, and key settings.
2. The API provisions Docker and starts `basse-agent` over SSH.
3. Later API calls use `ssh -L` to reach `http://127.0.0.1:8888` on the server.
4. The local agent API handles Docker, app containers, and Caddy proxy updates.

This mode supports host-level operations such as provisioning, connection
checks, agent container logs/stats, and selected-server source builds.

### Outbound mode

Outbound mode is for locked-down servers where Basse cannot connect inward:

1. A user creates a server and chooses **Outbound agent**.
2. The API creates an encrypted agent token, stores `connectionMode=outbound`,
   and returns a one-time `docker run` command.
3. The user runs that command on the server.
4. The agent starts its normal local HTTP server and a poller.
5. The poller sends `POST /api/agent/outbound/poll` to the control plane.
6. If a command is queued, the API returns `{ id, method, path, body }`.
7. The agent calls its own local endpoint, for example
   `POST http://127.0.0.1:8888/v1/apps/deploy`.
8. The agent posts the result to
   `POST /api/agent/outbound/commands/:id/result`.

From the rest of the API, outbound calls look like normal agent calls. The
transport layer decides whether to use an SSH tunnel or the outbound command
queue.

Outbound currently supports agent-backed operations: health checks, info, proxy
ensure/sync, app deploy, app status, app metrics, app logs, app console exec,
container import, and app container removal.

These still require SSH today:

- provisioning Docker and the agent
- raw SSH connection checks
- host-level `basse-agent` logs and Docker stats
- automatic agent image updates
- selected-server source builds
- app stop through the host shell

For source-based apps on outbound servers, use Depot builds. Prebuilt images and
managed database images deploy through the agent.

## Set up without SSH

Use outbound mode when the server can make HTTPS requests to Basse, but Basse
cannot connect back to the server over SSH or any public ingress port.

### Requirements

- Docker is already installed and running on the target server.
- The user running the install command can mount `/var/run/docker.sock`.
- The Basse API is reachable from the server over HTTPS.
- `API_ORIGIN` is set correctly for the API process in production. This is what
  the dashboard uses when it generates the outbound install command.
- For source-based app builds, Depot is connected. Selected-server builds still
  need SSH because they upload and build the source tree on the host over SSH.

### Cloud setup

1. Set the public API origin on the API service:

   ```sh
   API_ORIGIN=https://basse.sh
   ```

2. Deploy the API and web app.
3. Open the Basse dashboard.
4. Go to **Servers**.
5. Choose **Add a server**.
6. Select **Outbound agent**.
7. Enter a server name and the server address. The address is used for display,
   DNS, and public database connection strings; it is not used for SSH.
8. Create the server.
9. Copy the generated install command.
10. Run it on the target server.
11. Wait for the server status to become active. The agent marks itself active
    when its first poll reaches the API.

The generated command includes the raw `BASSE_AGENT_TOKEN`. Treat it like a
secret. It is shown once by the dashboard, then only an encrypted copy and a
lookup hash are stored by the API.

### Local development setup

For local development, the server still needs to reach your local API. The
simple path is an HTTPS tunnel to the API port:

```sh
# terminal 1
bun run dev:api

# terminal 2, using any HTTPS tunnel provider
cloudflared tunnel --url http://127.0.0.1:3000
```

Start the API with `API_ORIGIN` set to the HTTPS tunnel URL before creating the
outbound server:

```sh
API_ORIGIN=https://your-tunnel.example.com bun run dev:api
```

Then create the server from the dashboard with **Outbound agent**, copy the
install command, and run it on a machine that can reach that tunnel URL.

### Verify it connected

On the server:

```sh
docker ps --filter name=basse-agent
docker logs --tail 100 basse-agent
```

In Basse:

- The server should move from pending to active.
- Server agent info should show Docker and engine details.
- Deploying a prebuilt image should pull and run through the agent.
- Domains should sync through the proxy agent endpoints.

If it stays pending, check that:

- `BASSE_CONTROL_PLANE_URL` in the container points at the public API origin.
- The API route `/api/agent/outbound/poll` is reachable from the server.
- The container has the expected `BASSE_AGENT_TOKEN`.
- The API process has the migration that creates `agent_command` and
  `server.connection_mode`.

## Endpoints

| Method | Path          | Auth   | Purpose                                       |
| ------ | ------------- | ------ | --------------------------------------------- |
| GET    | `/healthz`    | none   | Liveness. Always 200 if the process is up.    |
| GET    | `/readyz`     | none   | Readiness. 200 if Docker reachable, else 503. |
| GET    | `/v1/info`    | bearer | Host + Docker facts.                          |
| GET    | `/v1/version` | bearer | Agent build version.                          |

## Configuration

| Env var               | Default                | Notes                        |
| --------------------- | ---------------------- | ---------------------------- |
| `BASSE_AGENT_TOKEN`   | —                      | Required. Bearer credential. |
| `BASSE_AGENT_MODE`    | `serve`                | `serve` or `outbound`.       |
| `BASSE_AGENT_PORT`    | `8888`                 | Listen port.                 |
| `BASSE_DOCKER_SOCKET` | `/var/run/docker.sock` | Docker Engine API socket.    |
| `BASSE_CONTROL_PLANE_URL` | —                  | Required in outbound mode. Base API origin, for example `https://basse.sh`. |
| `BASSE_CADDY_IMAGE` | `caddy:2` | Caddy image the proxy runs. |
| `BASSE_CADDY_CONTAINER` | `basse-caddy` | Proxy container name. |
| `BASSE_PROXY_NETWORK` | `basse` | Docker network shared by apps and Caddy. |
| `BASSE_CADDY_DATA_VOLUME` | `basse_caddy_data` | Caddy data volume for state and certificates. |
| `BASSE_CADDY_ADMIN_VOLUME` | `basse_caddy_admin` | Shared volume for the Caddy admin unix socket. |
| `BASSE_CADDY_ADMIN_DIR` | `/run/caddy-admin` | Mount path for the admin socket volume. |

## Commands

- `agent serve` (default) — run the HTTP server.
- `agent healthcheck` — self-probe `/healthz`; used by the container HEALTHCHECK.
- `agent version` — print the build version.

## Build

```sh
go build ./...
go vet ./...

# Image
docker build -t basse-agent --build-arg VERSION=$(git rev-parse --short HEAD) .
```

## How it is run on a server

In SSH mode, the control plane provisions it during server bootstrap:

```sh
docker run -d --name basse-agent --restart unless-stopped \
  -p 127.0.0.1:8888:8888 \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --env-file /etc/basse/agent.env \
  ghcr.io/lassejlv/basse-agent:latest
```

In outbound mode, the dashboard returns a one-time command shaped like this:

```sh
docker run -d --name basse-agent --restart unless-stopped \
  -v /var/run/docker.sock:/var/run/docker.sock \
  -v basse_caddy_data:/data \
  -v basse_caddy_admin:/run/caddy-admin \
  -e BASSE_AGENT_TOKEN='<one-time-token-from-basse>' \
  -e BASSE_AGENT_MODE=outbound \
  -e BASSE_CONTROL_PLANE_URL='https://basse.sh' \
  ghcr.io/lassejlv/basse-agent:latest
```

`BASSE_CONTROL_PLANE_URL` should be the public API origin. In cloud/prod this is
usually derived from `API_ORIGIN`; locally it can be the HTTPS tunnel URL that
reaches the API.
