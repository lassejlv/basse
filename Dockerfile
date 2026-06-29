FROM oven/bun:latest AS base
WORKDIR /app

FROM base AS deps
COPY package.json bunfig.toml ./
COPY apps/api/package.json ./apps/api/
COPY apps/web/package.json ./apps/web/
COPY packages/db/package.json ./packages/db/
RUN bun install

FROM deps AS build
COPY . .
RUN bun run --cwd apps/web build

FROM base AS runner
ENV NODE_ENV=production
ENV API_HOST=0.0.0.0
ENV API_PORT=3000
COPY --from=build /app ./
EXPOSE 3000
# Apply pending migrations, then start the server. exec keeps the server as PID 1
# so it receives signals (graceful shutdown) correctly.
CMD ["sh", "-c", "bun run --cwd packages/db db:migrate:ci && exec bun apps/api/src/index.ts"]
