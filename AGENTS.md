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
