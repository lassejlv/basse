# Repository Guidelines

## Project Structure & Module Organization

Basse is a Bun workspace for a self-hosted PaaS control plane plus a Go server agent.

- `apps/api/src/` contains the Hono API, deployment workflows, queues, SSH/provisioning, and Better Auth entrypoints.
- `apps/web/src/` contains the React/Vite UI. Routes live in `apps/web/src/routes/`, shared API clients in `apps/web/src/lib/`, and reusable UI in `apps/web/src/components/`.
- `apps/agent/` is the dependency-free Go agent that runs on user servers and talks to Docker/Caddy.
- `packages/db/` owns Drizzle schema, migrations, DB client setup, auth schema, and workspace helpers. Keep DB/auth ownership here.
- `packages/shared/` is for cross-app TypeScript types and helpers.
- `coolify.md` tracks product parity and shipped progress.

## Build, Test, and Development Commands

- `bun install` installs workspace dependencies using the isolated linker.
- `bun run dev` starts all workspace dev processes.
- `bun run dev:api` runs the API with `.env` loaded.
- `bun run dev:web` runs the Vite UI on `127.0.0.1`.
- `bun run check` runs TypeScript checks across workspaces.
- `bun run lint` runs `oxlint .`.
- `bun run format` applies `oxfmt --write .`.
- `bun run db:generate` creates Drizzle migrations from schema changes.
- `bun run db:migrate` applies Drizzle migrations.
- In `apps/agent/`: `go build ./...` and `go vet ./...`.

## Coding Style & Naming Conventions

Prefer TypeScript modules with explicit exports and small files grouped by feature. Use `camelCase` for functions and variables, `PascalCase` for React components, and TanStack Router filenames such as `_authed/projects.$projectId.tsx`. Run `bun run format` before committing. For Go, keep the current standard-library-only agent design unless a dependency is clearly justified.

## Testing Guidelines

There is no broad test suite yet. Treat `bun run check`, `bun run lint`, and `go vet ./...` as minimum validation. Add focused tests beside risky logic, named by feature intent, such as `deployments.test.ts` or `server_test.go`.

## Commit & Pull Request Guidelines

Recent commits use short imperative subjects, for example `Add app resource limits` or `Move deployment logs into detail sheets`. Keep commits scoped and describe behavior. Pull requests should include the user-visible change, validation commands, linked issues when relevant, and screenshots or clips for UI changes.

## Security & Configuration Tips

Do not commit `.env` files, tokens, SSH keys, or generated server credentials. The agent API requires bearer auth and is intended for loopback exposure behind SSH tunnels. Keep migrations in `packages/db/drizzle/` and review destructive schema changes carefully.

## Cursor Cloud specific instructions

Runtime is **Bun** (symlinked at `/usr/local/bin/bun`); Postgres 16 and Redis 7 are installed. The update script runs `bun install` on startup, but the following are NOT automatic and must be done each session before running the app:

- **Start services (no systemd):** `sudo pg_ctlcluster 16 main start` and `sudo redis-server --daemonize yes`. Verify with `redis-cli ping` (expect `PONG`) and `pg_lsclusters`.
- **`.env` is git-ignored and not persisted.** If `/workspace/.env` is missing, recreate it: `cp .env.example .env` then set a real `BETTER_AUTH_SECRET` (≥32 chars, e.g. `bun run auth:secret` or `openssl rand -hex 32`). The defaults for `DATABASE_URL` (`postgres://postgres:postgres@127.0.0.1:5432/basse`) and `REDIS_URL` match the local services below.
- **Postgres role/db:** the `postgres` role password is set to `postgres` and a `basse` database exists. If starting from a fresh DB, run `sudo -u postgres psql -c "ALTER USER postgres WITH PASSWORD 'postgres';"` and `sudo -u postgres createdb basse`, then `bun run db:migrate`.
- **Migrations** must be applied after schema changes / fresh DB: `bun run db:migrate` (harmless `identifier ... will be truncated` NOTICEs are expected).

Standard commands are in the "Build, Test, and Development Commands" section above (`bun run dev`, `bun run check`, `bun run lint`, `bun test`). Dev servers: API on `127.0.0.1:3000`, web (Vite) on `127.0.0.1:5173` which proxies `/api` + `/health` to the API.

Non-obvious gotchas:

- **Email is disabled locally**, so signup/login OTP codes are NOT emailed — they are logged to the API console as `[email] OTP delivery disabled; code for <email> (<type>): <code>`. Only the most recently sent code is valid; requesting a new one invalidates the previous. To verify an account without the UI: POST `/api/auth/email-otp/send-verification-otp` then POST `/api/auth/email-otp/verify-email` with the freshest logged code.
- **Redis is required for real end-to-end flows** (BullMQ worker/monitor for provisioning, deployments, monitoring). The API process will still boot without Redis, but those background flows silently won't run.
- The **Go agent** (`apps/agent/`) is optional (runs on remote user servers, not the control plane) and its `go.mod` targets Go 1.26; the VM has Go 1.22, so its toolchain-download build/vet may fail here. This does not affect the control plane.
