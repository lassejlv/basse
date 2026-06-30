FROM oven/bun:latest AS base
WORKDIR /app

FROM base AS deps
COPY package.json bun.lock bunfig.toml ./
COPY apps/api/package.json ./apps/api/
COPY apps/web/package.json ./apps/web/
COPY packages/db/package.json ./packages/db/
COPY packages/shared/package.json ./packages/shared/
RUN bun install --frozen-lockfile

FROM deps AS build
COPY . .
RUN bun run --cwd apps/web build

FROM base AS runner
ENV NODE_ENV=production
ENV API_HOST=0.0.0.0
ENV API_PORT=3000
# Pin the Railpack CLI + matching BuildKit frontend image (must agree).
ENV RAILPACK_VERSION=0.30.0
ENV BASSE_RAILPACK_FRONTEND=ghcr.io/railwayapp/railpack-frontend:v0.30.0
# Tooling the deploy pipeline shells out to:
# - openssh-client: ssh/ssh-keygen for server provisioning.
# - git: clone public repos to build.
# - curl + ca-certificates: fetch the depot/railpack installers (not in the bun base).
# Then install the Depot CLI (remote builder) and the Railpack CLI (no-Dockerfile builds).
RUN apt-get update \
  && apt-get install -y --no-install-recommends openssh-client git ca-certificates curl \
  && rm -rf /var/lib/apt/lists/* \
  && curl -L https://depot.dev/install-cli.sh | DEPOT_INSTALL_DIR=/usr/local/bin sh \
  && curl -fsSL "https://github.com/railwayapp/railpack/releases/download/v${RAILPACK_VERSION}/railpack-v${RAILPACK_VERSION}-x86_64-unknown-linux-musl.tar.gz" \
     | tar -xz -C /usr/local/bin railpack \
  && depot --version && railpack --version
COPY --from=build /app ./
EXPOSE 3000
# Apply pending migrations, then start the server. exec keeps the server as PID 1
# so it receives signals (graceful shutdown) correctly.
CMD ["sh", "-c", "bun run --cwd packages/db db:migrate:run && exec bun apps/api/src/index.ts"]
