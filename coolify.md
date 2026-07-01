# Coolify Feature Inventory

Last checked: 2026-06-30

This is a product reference for building a Coolify-like platform. It focuses on user-facing and operator-facing capabilities, not Coolify's internal implementation.

## Basse Progress

Checked against the current repo on 2026-06-30.

### Done

- [x] Set up Bun monorepo with `apps/*` and `packages/*` workspaces.
- [x] Added `bunfig.toml` with isolated linker enabled.
- [x] Added root scripts for dev, API dev, web dev, checks, linting, formatting, and DB migrations.
- [x] Added `apps/api` TypeScript service.
- [x] Added Hono API entrypoint.
- [x] Added `/health` API route.
- [x] Mounted Better Auth under `/api/auth/*`.
- [x] Added Better Auth email/password auth.
- [x] Added Better Auth organization plugin.
- [x] Added personal workspace creation on account signup.
- [x] Added API CORS/trusted origin handling for the Vite dev server.
- [x] Added `packages/db` for database ownership.
- [x] Added Drizzle schema package.
- [x] Switched runtime database adapter to Drizzle `bun-sql`.
- [x] Configured PostgreSQL/Neon support for dev.
- [x] Disabled prepared statements for Neon pooler compatibility.
- [x] Generated Better Auth tables with the Better Auth CLI.
- [x] Added Basse schema tables for servers, projects, apps, and deployments.
- [x] Generated and applied Drizzle migrations.
- [x] Added `apps/web` Vite React app.
- [x] Added Tailwind CSS v4.
- [x] Added Coss/shadcn UI components, including the sidebar and avatar primitives.
- [x] Added TanStack Query provider.
- [x] Added TanStack Router file-based routing.
- [x] Added login and signup pages.
- [x] Added Vite reverse proxy for `/api` and `/health`.
- [x] Added pathless `_authed` route for protected dashboard pages.
- [x] Moved dashboard to `/dashboard`.
- [x] Added dashboard sidebar shell using `apps/web/src/components/ui/sidebar.tsx`.
- [x] Added organization/workspace selector to the dashboard sidebar.
- [x] Removed login/signup buttons from dashboard navigation.
- [x] Added user avatar, name, and email in the bottom of the dashboard sidebar.
- [x] Reduced dashboard content to a simple overview heading and description.
- [x] Kept the public root `/` redirecting to `/dashboard`.
- [x] Added workspace traffic-provider integrations for managed load balancers, starting with Hetzner Cloud.
- [x] Added managed load balancer and load balancer target records in `packages/db`.
- [x] Added Hetzner Cloud load balancer creation/sync with 80/443 passthrough services, health checks, endpoint capture, and target tracking.
- [x] Added Cloudflare Load Balancers creation/sync with token validation, zone discovery, monitors, pools, proxied hostnames, and target tracking.
- [x] Relaxed domain uniqueness to allow the same host on multiple target servers behind a managed load balancer.
- [x] Added dashboard flows to connect Hetzner or Cloudflare and attach a managed load balancer to multi-server service apps.
- [x] Added staged change history so applied and discarded config/env changes remain visible after the pending set is cleared.
- [x] Made staged changes project-wide, with project-level review, apply, discard, and history across apps and environments.
- [x] Added project shared and environment shared variables with `{{shared.KEY}}` / `{{env.KEY}}` references, autocomplete, and deploy-time resolution.
- [x] Added background monitoring for server reachability, stuck deployments, app container health, resource pressure, and in-app alerts.
- [x] Added Cloudflare Email Sending delivery for new monitor alerts through `@opencoredev/email-sdk`.
- [x] Added `packages/emails` with React Email templates for monitor alert email rendering.
- [x] Added GitHub App manifest setup, installation callback handling, installed repository picker, and private repository clone support through short-lived installation tokens.

### Current Validation

- [x] `bun run check`
- [x] `bun run test`
- [x] `bun run lint`
- [x] `bun run --cwd apps/web build`
- [x] `git diff --check`

### Next Implementation Targets

- [ ] Add automatic managed load balancer resync when app server assignments change.
- [ ] Add provider DNS automation once Basse owns authoritative DNS settings.
- [ ] Add load balancer action history and health telemetry.
- [ ] Add real dashboard data fetching through TanStack Query.
- [ ] Add API routes for projects, servers, apps, and deployments.
- [ ] Add authenticated API middleware.
- [ ] Add organization-aware ownership to Basse resources.
- [ ] Add create-project and create-app flows.
- [ ] Add first deploy job model and agent contract.

## Product Shape

Coolify is a self-hosted platform-as-a-service for deploying apps, databases, and Docker-compatible services to one or more servers.

Core promise:

- Bring your own server.
- Connect Git or Docker image sources.
- Deploy apps, databases, and one-click services.
- Route traffic through a managed reverse proxy.
- Manage environment variables, domains, persistent storage, logs, backups, and notifications from a dashboard.
- Expose an API for automation.

## Resource Model

Coolify organizes resources roughly like this:

- Team
- Project
- Environment, for example production or staging
- Server
- Resource
  - Application
  - Database
  - Service stack

Basse should probably model this early, even if v0 only has one user and one server. The hierarchy matters once shared variables, preview environments, access control, and API tokens exist.

## Server Management

Features:

- Add and manage one or multiple servers.
- Deploy resources to a selected server.
- Support self-hosted servers and cloud/VPS providers.
- Server reachability checks.
- Server disk usage notifications.
- Docker cleanup events.
- Server patching notifications.
- Terminal access for servers and containers, gated by admin/security controls.
- Proxy management for routed resources.
- Support for Docker-compatible workloads.

Basse parity:

- v0: single server registration with agent health.
- v1: multi-server inventory, server labels/capacity, remote Docker/agent control.
- later: server patching, maintenance windows, server terminal, Docker cleanup policy.

## Application Sources

Features:

- Git-based deployment.
- Push-to-deploy workflow.
- Hosted and self-hosted Git providers such as GitHub, GitLab, Bitbucket, Gitea, and others.
- Docker image deployment from a registry.
- Dockerfile deployment.
- Docker Compose deployment for multi-service apps.
- Static site / SPA deployment.
- Nixpacks automatic build detection.

Supported build modes:

- Nixpacks
- Static
- Dockerfile
- Docker Compose
- Docker Image

Basse parity:

- v0: Git repo URL + branch + Dockerfile or Nixpacks.
- v0.5: GitHub App integration for workspace-owned private repository access, installation tracking, installed repository picker, and short-lived installation-token clones.
- v1: Docker image deploys, static sites, compose stacks.
- later: GitLab/Bitbucket/Gitea provider apps, deploy keys, private repo OAuth, monorepo base directory detection.

## Deployment Flow

Features:

- Manual deploy.
- Push-to-deploy from Git events.
- Build logs.
- Runtime logs.
- Deployment status tracking.
- Deployment success/failure notifications.
- Container status change notifications.
- Health-check-gated deploys.
- Rolling updates for eligible app deployments.
- Old container remains live while new container starts.
- New container replaces old container after it is healthy.
- Rollout troubleshooting via proxy and container logs.

Rolling update constraints:

- Requires a valid passing health check.
- Requires default container naming.
- Not supported for Docker Compose deployments.
- Host port mapping can prevent rolling updates because the new and old container cannot bind the same host port.

Basse parity:

- v0: deployment row + agent job + logs + final status.
- v1: health-check-gated cutover.
- v1.5: rollback to previous image.
- later: true rolling updates, deployment queue, cancel/retry, build cache controls.

## Health Checks

Features:

- Resource health checks to ensure traffic routes only to healthy resources.
- UI-configured application health checks.
- Dockerfile `HEALTHCHECK` support.
- Docker Compose `healthcheck` support for compose/service stacks.
- Expected path/status/interval configuration in UI.
- Health checks integrate with Traefik routing.
- If health checks fail, proxy may stop routing traffic to the resource.
- Health checks are recommended for all resources but can be disabled.

Important behavior:

- Dockerfile health checks take precedence when both UI and Dockerfile health checks are enabled.
- UI health checks require `curl` or `wget` in the container.

Basse parity:

- v0: HTTP path + expected status + interval + timeout.
- v1: Dockerfile/compose healthcheck detection.
- later: readiness vs liveness distinction, health history, health event timeline.

## Domains And Proxy

Features:

- Public FQDN assignment for apps/services.
- Automatic proxy routing to healthy resources.
- Proxy-aware health checks.
- Traefik proxy support in documented behavior.
- Proxy logs used for troubleshooting deploy/routing issues.
- Custom domains per resource.
- Multiple domains through FQDN-style configuration.

Basse parity:

- v0: one domain per single-server app, Caddy route generation, managed Hetzner and Cloudflare load balancers for multi-server service apps.
- v1: multiple domains, TLS automation, redirects, automatic load balancer resync on target changes.
- later: wildcard domains, custom headers, middleware, proxy logs UI, broader provider DNS automation.

## Environment Variables And Secrets

Features:

- Environment variables per resource.
- Preview deployments can have different environment variables from production.
- Normal UI editor.
- Developer `.env`-style bulk editor.
- Build-time vs runtime variable flags.
- Build variables injected during image build.
- Runtime variables injected into running containers.
- Dockerfile build secrets via BuildKit secrets.
- Docker Compose secrets integration.
- Multiline variables.
- Literal variables.
- Locked secrets.
- Shared variables at team, project, and environment level.
- Template syntax for shared variables, such as `{{team.KEY}}`, `{{project.KEY}}`, and `{{environment.KEY}}`.
- Predefined app variables such as:
  - `COOLIFY_FQDN`
  - `COOLIFY_URL`
  - `COOLIFY_BRANCH`
  - `COOLIFY_RESOURCE_UUID`
  - `COOLIFY_CONTAINER_NAME`
  - `SOURCE_COMMIT`
  - `PORT`
  - `HOST`
- Service stack variables such as `SERVICE_NAME_<ID>`.
- Magic environment variables for Docker Compose / service stacks, using `SERVICE_<TYPE>_<IDENTIFIER>` syntax for generated URLs, FQDNs, passwords, and random strings.
- Generated magic values persist between deployments and can be reused across services.

Basse parity:

- v0: encrypted runtime env vars per app.
- v1: build/runtime split, shared project/env variables.
- later: locked secrets, magic variables, secret rotation, env diff preview before deploy.

## Persistent Storage

Features:

- Persistent volumes for apps and services.
- Volume configuration for Docker-compatible resources.
- Storage preservation across deployments.
- Compose/service-stack storage mapping.

Basse parity:

- v0: named Docker volumes per app.
- v1: bind mount vs named volume selection, backup labels.
- later: volume browser, migration, snapshot/restore.

## Databases

Features:

- One-click database provisioning.
- Database resources as first-class deployable resources.
- Supported databases documented by Coolify include:
  - PostgreSQL
  - MySQL
  - MariaDB
  - MongoDB
  - Redis
  - KeyDB
  - DragonFly
  - ClickHouse
- Database connection details surfaced to users.
- Database backups for supported engines.
- Backup success/failure notifications.

Basse parity:

- v0: PostgreSQL only.
- v1: Redis, MySQL/MariaDB.
- later: MongoDB, ClickHouse, logical backup scheduling, point-in-time-ish restore story where possible.

## Backups

Features:

- Automated Coolify instance backups.
- AWS S3 backup setup.
- S3-compatible backup strategy.
- Database backup workflows.
- Backup success notifications.
- Backup failure notifications.
- Lifecycle/cost guidance through S3 lifecycle rules.

Basse parity:

- v0: database dump to local disk. DONE — pg_dump via agent onto the database's data volume, manual + scheduled (per-app interval), retention pruning, restore, streamed download, success/failure alerts.
- v1: S3-compatible destination. DONE — workspace S3 connections (`/s3` page, encrypted credentials, Bun native S3 client), auto/manual upload per backup, S3 download fallback, object cleanup on delete.
- later: encrypted backups, restore drill UI, per-resource cron expressions, backups for Redis/MySQL, downloads/uploads for outbound-mode servers.

## Logs And Observability

Features:

- Build logs.
- Application container logs.
- Proxy logs for routing/deployment debugging.
- Drain logs to external destinations.
- Documented log drain targets include Axiom, New Relic, and custom Fluent Bit destinations.
- Container status change events.
- Server reachability and disk usage events.
- Monitoring services available through one-click catalog, for example Uptime Kuma, Grafana, SigNoz, Beszel, Glances, Checkmate, and others.

Basse parity:

- v0: deployment logs streamed from agent to API.
- v1: app logs tail.
- later: log drains, metrics, alerts, retention/search.

## Scheduled Tasks And Cron

Features:

- Scheduled task success notifications.
- Scheduled task failure notifications.
- Supported cron syntax documented in the knowledge base.

Basse parity:

- v0: defer.
- v1: cron jobs per app, logs and exit status.
- later: run-once jobs, retries, schedules per environment.

## Notifications

Features:

- Notification settings in dashboard.
- Multiple notification channels.
- Different events can be routed to different channels.
- Webhook notifications via HTTP/HTTPS POST.
- Pushover notifications.
- Email/Slack-style channels are part of the broader notification system.

Notification event groups:

- Deployments
  - Deployment success
  - Deployment failure
  - Container status changes
- Backups
  - Backup success
  - Backup failure
- Scheduled tasks
  - Scheduled task success
  - Scheduled task failure
- Server
  - Docker cleanup success
  - Docker cleanup failure
  - Server disk usage
  - Server reachable
  - Server unreachable
  - Server patching
  - Traefik proxy outdated

Basse parity:

- v0: webhook on deploy success/failure.
- v1: email and Slack-compatible webhook.
- later: per-event routing, notification test button, delivery logs.

## API And Automation

Features:

- API reference.
- Bearer-token authentication.
- API tokens created in Keys & Tokens / API tokens.
- Tokens scoped to a single team.
- Token permissions determine available data and actions.
- CLI can interact with Coolify API when host/token are configured.

Basse parity:

- v0: internal API only.
- v1: personal/team API tokens with scoped permissions.
- later: generated OpenAPI spec, CLI, webhooks, audit log for API actions.

## Teams And Access Control

Features:

- Team-scoped API tokens.
- Team-level shared variables.
- Team ownership boundary for projects/resources.
- Permissioned actions implied by token scopes.

Basse parity:

- v0: single owner.
- v1: teams, invites, roles.
- later: resource-level permissions, audit log, SSO/OIDC.

## One-Click Services

Coolify has a large one-click services catalog. Current categories include:

- Administration
- AI
- Analytics
- Automation
- Backup
- Bookmarks
- Browser
- Business
- CMS
- Communication
- Crypto
- Database
- Design
- Development
- Documentation
- Education
- Email
- Family
- File Management
- File Sharing
- Finance
- Forum
- Gaming
- Health
- Home
- IoT
- Marketing
- Media
- Monitoring
- Networking
- Notifications
- Productivity
- Project Management
- RSS
- Search
- Security
- Social Media
- Storage
- Utilities

Notable service examples:

- WordPress, Ghost, Drupal, Joomla, Strapi, Directus
- Supabase, PocketBase, Appwrite, NocoDB
- PostgreSQL-adjacent tools like pgAdmin and PG Back Web
- Redis Insight, RabbitMQ, Docker Registry
- Grafana, Uptime Kuma, SigNoz, Beszel
- N8N, ActivePieces, Trigger
- MinIO, NextCloud, OwnCloud, Seafile
- Gitea, Forgejo, GitLab, Jenkins, GitHub Runner
- Plausible, PostHog, Umami, Metabase, Superset
- Keycloak, Authentik, Vaultwarden, Infisical
- Ollama, Open WebUI, LibreChat, Langfuse, Qdrant, Weaviate

Basse parity:

- v0: no one-click catalog.
- v1: template format and 3-5 first-party templates.
- later: community template registry, service version pinning, template updates, template diff preview.

## Coolify Cloud / Hosting Shape

Features:

- Coolify can be self-hosted.
- Coolify also offers a cloud-managed experience.
- Resources can be deployed across a single server or multiple servers depending on requirements.

Basse parity:

- v0: self-hosted only.
- later: managed control plane with user-owned agents.

## Missing / Open Questions For Basse

These need product decisions before implementation:

- Do we build around Docker Engine only, or keep an abstraction for Podman/Firecracker later?
- Should the control plane run on the same server as the agent in v0?
- Do we use Caddy instead of Traefik? Current Basse direction says yes.
- Do we support Docker Compose early, or keep v0 to one-container apps?
- How much of the one-click service catalog should exist before app deploys are excellent?
- Do preview deployments create separate resource records, or are they child deployments of an app?
- Do environment variables inherit with override rules or get flattened at deploy time?
- What is the minimum backup promise we are willing to make without lying to users?

## Suggested Basse Milestones

### Milestone 0: Sharp Deploy Loop

- Server agent installed and reachable.
- Create app with repo URL, branch, build mode, domain, env vars.
- Deploy app.
- Stream build/deploy logs.
- Route domain through Caddy.
- Health check app.
- Show final deployment status.

### Milestone 1: Operable PaaS

- Multiple apps per server.
- Persistent volumes.
- PostgreSQL resource.
- App logs.
- Manual redeploy and rollback.
- Deploy webhooks.
- Webhook notifications.
- Basic API tokens.

### Milestone 2: Team-Ready

- Teams/projects/environments.
- Shared variables.
- Build/runtime env split.
- Private Git integration.
- S3-compatible backups.
- Scheduled jobs.
- Audit log.

### Milestone 3: Coolify-Class Surface

- Docker Compose stacks.
- One-click service templates.
- Preview deployments.
- Rolling updates.
- Multi-server scheduling.
- Log drains.
- Terminal access.
- Full public API and CLI.

## Sources

- Coolify homepage: https://coolify.io/
- Applications docs: https://coolify.io/docs/applications
- Environment variables docs: https://coolify.io/docs/knowledge-base/environment-variables
- Health checks docs: https://coolify.io/docs/knowledge-base/health-checks
- Rolling updates docs: https://coolify.io/docs/knowledge-base/rolling-updates
- Notifications docs: https://coolify.io/docs/knowledge-base/notifications
- S3 backup docs: https://coolify.io/docs/knowledge-base/s3/aws
- S3 introduction / terminal access index: https://coolify.io/docs/knowledge-base/s3/introduction
- Services overview: https://coolify.io/docs/services/overview
- All services directory: https://coolify.io/docs/services/all
- API authorization docs: https://coolify.io/docs/api-reference/authorization
