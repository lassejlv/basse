# Basse agent

The on-server agent. It runs as a container on each user server, mounts the
Docker socket, and exposes a small bearer-authenticated HTTP API that the Basse
control plane reaches over an SSH tunnel.

## Why Go (and no dependencies)

The agent is a single static binary with **zero external dependencies** — it
talks to the Docker Engine API directly over the mounted unix socket using only
the standard library (`internal/dockerx`). That keeps the image tiny and the
build hermetic (no `go.sum`, no module downloads).

## Transport / security

- The container publishes its port to **host loopback only**
  (`docker run -p 127.0.0.1:8888:8888`). There is no public port.
- The control plane reaches the agent over an on-demand `ssh -L` tunnel reusing
  the per-server provisioning key, so the bearer token never crosses the network.
- `/v1/*` routes require `Authorization: Bearer <BASSE_AGENT_TOKEN>`, compared in
  constant time. The agent refuses to start if the token is empty.
- It runs as **root** so it can read `/var/run/docker.sock` (owned `root:docker`).

## Endpoints

| Method | Path          | Auth   | Purpose                                  |
| ------ | ------------- | ------ | ---------------------------------------- |
| GET    | `/healthz`    | none   | Liveness. Always 200 if the process is up. |
| GET    | `/readyz`     | none   | Readiness. 200 if Docker reachable, else 503. |
| GET    | `/v1/info`    | bearer | Host + Docker facts.                     |
| GET    | `/v1/version` | bearer | Agent build version.                     |

## Configuration

| Env var               | Default                  | Notes                            |
| --------------------- | ------------------------ | -------------------------------- |
| `BASSE_AGENT_TOKEN`   | —                        | Required. Bearer credential.     |
| `BASSE_AGENT_PORT`    | `8888`                   | Listen port.                     |
| `BASSE_DOCKER_SOCKET` | `/var/run/docker.sock`   | Docker Engine API socket.        |

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

The control plane provisions it during server bootstrap:

```sh
docker run -d --name basse-agent --restart unless-stopped \
  -p 127.0.0.1:8888:8888 \
  -v /var/run/docker.sock:/var/run/docker.sock \
  --env-file /etc/basse/agent.env \
  ghcr.io/lassejlv/basse-agent:latest
```
