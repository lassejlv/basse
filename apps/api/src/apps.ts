import { app, appServer, db, deployment, environment, project, server } from "@basse/db";
import type {
  App,
  AppBuildMode,
  AppBuildRunner,
  AppKind,
  AppConsoleResult,
  DatabaseConnectionInfo,
  DatabaseKind,
  AppLogs,
  AppMetrics,
  AppSourceType,
  AppVolume,
  CreateAppInput,
  DeploymentStatus,
  UpdateAppInput,
} from "@basse/shared";
import { and, desc, eq, inArray } from "drizzle-orm";
import { Hono } from "hono";
import { removeAppContainers } from "./app-cleanup";
import {
  execAppCommand,
  getAppLogs as getAgentAppLogs,
  getAppMetrics as getAgentAppMetrics,
} from "./agent-client";
import { decryptSecret, encryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";
import { runScript } from "./ssh";
import { resolveActiveWorkspace } from "./workspace";

type AppRow = typeof app.$inferSelect;

const BUILD_MODES: AppBuildMode[] = ["auto", "dockerfile", "railpack"];
const BUILD_RUNNERS: AppBuildRunner[] = ["depot", "server"];
const APP_KINDS: AppKind[] = ["service", "database"];
const SOURCE_TYPES: AppSourceType[] = ["repository", "image"];
const DATABASE_KINDS: DatabaseKind[] = ["postgres", "redis"];
const DEFAULT_POSTGRES_VERSION = "18";
const DEFAULT_REDIS_VERSION = "8";
const POSTGRES_PORT = 5432;
const REDIS_PORT = 6379;

function slugify(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

function databaseIdentifier(value: string, fallback: string): string {
  const normalized = value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_]+/g, "_")
    .replace(/^_+|_+$/g, "");
  return normalized || fallback;
}

function databasePassword(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(24));
  return Buffer.from(bytes).toString("base64url");
}

function databaseInternalHost(appId: string): string {
  return `basse-app-${appId}`;
}

function databaseImage(kind: DatabaseKind, version: string): string {
  return kind === "postgres" ? `postgres:${version}` : `redis:${version}`;
}

function defaultDatabaseVersion(kind: DatabaseKind): string {
  return kind === "postgres" ? DEFAULT_POSTGRES_VERSION : DEFAULT_REDIS_VERSION;
}

function databasePort(kind: DatabaseKind): number {
  return kind === "postgres" ? POSTGRES_PORT : REDIS_PORT;
}

function validateDatabaseVersion(version: string): string | null {
  if (!version) return "databaseVersion is required";
  if (version.length > 32 || !/^[a-zA-Z0-9][a-zA-Z0-9._-]*$/.test(version)) {
    return "databaseVersion must be a valid Docker tag";
  }
  return null;
}

function validatePublicPort(port: number | null): string | null {
  if (port === null) return null;
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return "databasePublicPort must be a valid port";
  }
  return null;
}

function postgresUri(input: {
  user: string;
  password: string;
  host: string;
  port: number;
  database: string;
}): string {
  const user = encodeURIComponent(input.user);
  const password = encodeURIComponent(input.password);
  const database = encodeURIComponent(input.database);
  return `postgresql://${user}:${password}@${input.host}:${input.port}/${database}`;
}

function redisUri(input: {
  password: string;
  host: string;
  port: number;
  database: string;
}): string {
  const password = encodeURIComponent(input.password);
  const database = encodeURIComponent(input.database);
  return `redis://:${password}@${input.host}:${input.port}/${database}`;
}

function databaseUri(
  kind: DatabaseKind,
  input: { user: string | null; password: string; host: string; port: number; database: string },
): string {
  return kind === "postgres"
    ? postgresUri({
        user: input.user ?? "postgres",
        password: input.password,
        host: input.host,
        port: input.port,
        database: input.database,
      })
    : redisUri({
        password: input.password,
        host: input.host,
        port: input.port,
        database: input.database,
      });
}

// Public https git URL with no embedded credentials (no userinfo '@').
function validateRepositoryUrl(url: string): string | null {
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "repositoryUrl must be a valid URL";
  }
  if (parsed.protocol !== "https:") return "repositoryUrl must be an https URL";
  if (parsed.username || parsed.password) return "repositoryUrl must not contain credentials";
  return null;
}

function validateImageRef(value: string): string | null {
  if (!value) return "imageRef is required";
  if (value.length > 255) return "imageRef is too long";
  if (/\s/.test(value)) return "imageRef must not contain whitespace";
  if (value.startsWith("-")) return "imageRef must be a Docker image reference";
  return null;
}

function normalizeVolumes(value: unknown): AppVolume[] | null {
  if (!Array.isArray(value)) return null;
  const volumes: AppVolume[] = [];
  for (const item of value) {
    if (!item || typeof item !== "object") return null;
    const candidate = item as Partial<AppVolume>;
    const hostPath = typeof candidate.hostPath === "string" ? candidate.hostPath.trim() : "";
    const containerPath =
      typeof candidate.containerPath === "string" ? candidate.containerPath.trim() : "";
    const readOnly = candidate.readOnly === true;
    if (!hostPath && !containerPath) continue;
    volumes.push({ hostPath, containerPath, readOnly });
  }
  return volumes;
}

function validateVolumes(volumes: AppVolume[]): string | null {
  if (volumes.length > 20) return "Too many volumes";
  for (const volume of volumes) {
    if (!volume.hostPath || !volume.hostPath.startsWith("/")) {
      return "Volume host paths must be absolute";
    }
    if (!volume.containerPath || !volume.containerPath.startsWith("/")) {
      return "Volume container paths must be absolute";
    }
    if (volume.hostPath.includes(":") || volume.containerPath.includes(":")) {
      return "Volume paths must not contain ':'";
    }
  }
  return null;
}

function parseVolumes(value: string): AppVolume[] {
  try {
    const parsed = JSON.parse(value) as unknown;
    const volumes = normalizeVolumes(parsed);
    return volumes ?? [];
  } catch {
    return [];
  }
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function toApp(
  row: AppRow,
  serverIds: string[] = row.serverId ? [row.serverId] : [],
  latestDeploymentStatus: DeploymentStatus | null = null,
): App {
  const kind = (row.databaseKind ?? "postgres") as DatabaseKind;
  const database =
    row.appKind === "database"
      ? {
          kind,
          version: row.databaseVersion ?? defaultDatabaseVersion(kind),
          name: row.databaseName ?? (kind === "postgres" ? "postgres" : "0"),
          user: kind === "postgres" ? (row.databaseUser ?? "postgres") : null,
          internalHost: databaseInternalHost(row.id),
          internalPort: databasePort(kind),
          publicEnabled: row.databasePublicEnabled,
          publicPort: row.databasePublicPort,
        }
      : null;

  return {
    id: row.id,
    environmentId: row.environmentId,
    serverIds,
    serverId: row.serverId,
    name: row.name,
    slug: row.slug,
    repositoryUrl: row.repositoryUrl,
    branch: row.branch,
    port: row.port,
    buildMode: row.buildMode,
    buildRunner: row.buildRunner,
    appKind: row.appKind,
    sourceType: row.sourceType,
    imageRef: row.imageRef,
    volumes: parseVolumes(row.volumes),
    database,
    latestDeploymentStatus,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

/** Most recent deployment status per app, for at-a-glance health on lists. */
async function loadLatestDeploymentStatus(
  appIds: string[],
): Promise<Map<string, DeploymentStatus>> {
  const ids = [...new Set(appIds)];
  const map = new Map<string, DeploymentStatus>();
  if (ids.length === 0) return map;

  const rows = await db
    .select({ appId: deployment.appId, status: deployment.status })
    .from(deployment)
    .where(inArray(deployment.appId, ids))
    .orderBy(desc(deployment.createdAt));
  // Rows are newest-first; the first one seen per app is its latest.
  for (const row of rows) {
    if (!map.has(row.appId)) map.set(row.appId, row.status);
  }
  return map;
}

async function loadAppServerIds(appIds: string[]): Promise<Map<string, string[]>> {
  const ids = [...new Set(appIds)];
  const map = new Map<string, string[]>();
  for (const id of ids) map.set(id, []);
  if (ids.length === 0) return map;

  const rows = await db
    .select({ appId: appServer.appId, serverId: appServer.serverId })
    .from(appServer)
    .where(inArray(appServer.appId, ids));
  for (const row of rows) {
    map.get(row.appId)?.push(row.serverId);
  }
  return map;
}

function normalizeServerIds(
  body: Partial<CreateAppInput | UpdateAppInput> | null,
): string[] | null {
  if (Array.isArray(body?.serverIds)) {
    return [
      ...new Set(
        body.serverIds.filter((id): id is string => typeof id === "string" && id.length > 0),
      ),
    ];
  }
  if (typeof body?.serverId === "string" && body.serverId) return [body.serverId];
  if (body && "serverId" in body && body.serverId === null) return [];
  return null;
}

/** Verifies an environment belongs to the active workspace (env->project->org). */
async function ownedEnvironmentId(
  environmentId: string,
  organizationId: string,
): Promise<string | null> {
  const [row] = await db
    .select({ id: environment.id })
    .from(environment)
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(environment.id, environmentId), eq(project.organizationId, organizationId)))
    .limit(1);
  return row?.id ?? null;
}

/** Loads an app only if it belongs to the active workspace (app->env->project->org). */
export async function ownedApp(appId: string, organizationId: string): Promise<AppRow | null> {
  const [row] = await db
    .select({ app })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(app.id, appId), eq(project.organizationId, organizationId)))
    .limit(1);
  return row?.app ?? null;
}

async function validateServersInOrg(serverIds: string[], organizationId: string): Promise<boolean> {
  if (serverIds.length === 0) return true;
  const rows = await db
    .select({ id: server.id })
    .from(server)
    .where(and(inArray(server.id, serverIds), eq(server.organizationId, organizationId)));
  return rows.length === serverIds.length;
}

async function resolveAttachedServer(appId: string, requestedServerId?: string) {
  const rows = await db
    .select({ server })
    .from(appServer)
    .innerJoin(server, eq(appServer.serverId, server.id))
    .where(eq(appServer.appId, appId));

  if (rows.length === 0) {
    return { error: "Attach at least one server to the app first" };
  }
  if (requestedServerId) {
    const row = rows.find((candidate) => candidate.server.id === requestedServerId);
    return row ? { server: row.server } : { error: "Server is not attached to this app" };
  }
  if (rows.length > 1) {
    return { error: "Choose a server for this app" };
  }
  return { server: rows[0]!.server };
}

async function requireAgentTarget(appId: string, requestedServerId?: string) {
  const resolved = await resolveAttachedServer(appId, requestedServerId);
  if (!resolved.server) return resolved;
  if (resolved.server.status !== "active" || !resolved.server.agentToken) {
    return { error: "Target server is not active" };
  }
  const connection = await connectionFromServer(resolved.server);
  const token = await decryptSecret(resolved.server.agentToken);
  return { server: resolved.server, connection, token };
}

export const apps = new Hono();

apps.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const environmentId = c.req.query("environmentId");
  if (!environmentId || !(await ownedEnvironmentId(environmentId, organizationId))) {
    return c.json({ error: "Environment not found" }, 404);
  }

  const rows = await db
    .select()
    .from(app)
    .where(eq(app.environmentId, environmentId))
    .orderBy(app.createdAt);

  const appIds = rows.map((row) => row.id);
  const serverIds = await loadAppServerIds(appIds);
  const statuses = await loadLatestDeploymentStatus(appIds);
  return c.json(rows.map((row) => toApp(row, serverIds.get(row.id), statuses.get(row.id) ?? null)));
});

apps.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedApp(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const serverIds = await loadAppServerIds([row.id]);
  const statuses = await loadLatestDeploymentStatus([row.id]);

  // Breadcrumb context: the environment and project this app lives under.
  const [context] = await db
    .select({
      environmentName: environment.name,
      projectId: project.id,
      projectName: project.name,
    })
    .from(environment)
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(environment.id, row.environmentId))
    .limit(1);

  return c.json({
    ...toApp(row, serverIds.get(row.id), statuses.get(row.id) ?? null),
    environmentName: context?.environmentName,
    projectId: context?.projectId,
    projectName: context?.projectName,
  });
});

apps.get("/:id/database/connection", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  if (row.appKind !== "database" || !row.databaseKind || !row.databasePassword) {
    return c.json({ error: "Database app not found" }, 404);
  }

  const kind = row.databaseKind;
  const password = await decryptSecret(row.databasePassword);
  const database = row.databaseName ?? (kind === "postgres" ? "postgres" : "0");
  const user = kind === "postgres" ? (row.databaseUser ?? "postgres") : null;
  const internalUri = databaseUri(kind, {
    user,
    password,
    host: databaseInternalHost(row.id),
    port: databasePort(kind),
    database,
  });

  const [target] = await db
    .select({ server })
    .from(appServer)
    .innerJoin(server, eq(appServer.serverId, server.id))
    .where(eq(appServer.appId, row.id))
    .limit(1);

  const publicUri =
    row.databasePublicEnabled && row.databasePublicPort && target?.server.sshHost
      ? databaseUri(kind, {
          user,
          password,
          host: target.server.sshHost,
          port: row.databasePublicPort,
          database,
        })
      : null;

  return c.json({ internalUri, publicUri } satisfies DatabaseConnectionInfo);
});

apps.get("/:id/metrics", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const target = await requireAgentTarget(appId, c.req.query("serverId"));
  if (!target.server) return c.json({ error: target.error }, 400);

  const metrics = await getAgentAppMetrics(target.connection!, target.token!, appId);
  return c.json({
    timestamp: new Date().toISOString(),
    ...metrics,
  } satisfies AppMetrics);
});

apps.get("/:id/logs", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const target = await requireAgentTarget(appId, c.req.query("serverId"));
  if (!target.server) return c.json({ error: target.error }, 400);

  const tail = Math.min(Math.max(Number(c.req.query("tail") ?? 250) || 250, 20), 1000);
  const logs = await getAgentAppLogs(target.connection!, target.token!, appId, tail);
  return c.json(logs satisfies AppLogs);
});

apps.post("/:id/console", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as {
    command?: unknown;
    serverId?: unknown;
  } | null;
  const command = typeof body?.command === "string" ? body.command.trim() : "";
  if (!command) return c.json({ error: "command is required" }, 400);
  if (command.length > 500) return c.json({ error: "command is too long" }, 400);

  const target = await requireAgentTarget(
    appId,
    typeof body?.serverId === "string" ? body.serverId : undefined,
  );
  if (!target.server) return c.json({ error: target.error }, 400);

  const result = await execAppCommand(target.connection!, target.token!, appId, command);
  return c.json(result satisfies AppConsoleResult);
});

apps.post("/:id/stop", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as { serverId?: unknown } | null;
  const target = await requireAgentTarget(
    appId,
    typeof body?.serverId === "string" ? body.serverId : undefined,
  );
  if (!target.server) return c.json({ error: target.error }, 400);

  const container = `basse-app-${appId}`;
  const result = await runScript(
    target.connection!,
    `docker stop --time 10 ${shellQuote(container)} >/dev/null 2>&1 || true`,
    { timeoutMs: 30_000 },
  );
  if (result.exitCode !== 0) return c.json({ error: "Could not stop container" }, 502);

  await db
    .update(deployment)
    .set({ status: "stopped", updatedAt: new Date() })
    .where(and(eq(deployment.appId, appId), eq(deployment.status, "healthy")));

  return c.json({ ok: true });
});

apps.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateAppInput> | null;
  const environmentId = typeof body?.environmentId === "string" ? body.environmentId : "";
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const appKind = APP_KINDS.includes(body?.appKind as AppKind)
    ? (body?.appKind as AppKind)
    : "service";
  const databaseKind = DATABASE_KINDS.includes(body?.databaseKind as DatabaseKind)
    ? (body?.databaseKind as DatabaseKind)
    : "postgres";
  const databaseVersion =
    typeof body?.databaseVersion === "string" && body.databaseVersion.trim()
      ? body.databaseVersion.trim()
      : defaultDatabaseVersion(databaseKind);
  const defaultDatabaseIdentifier = databaseIdentifier(slugify(name).replaceAll("-", "_"), "app");
  const databaseName =
    databaseKind === "postgres"
      ? databaseIdentifier(
          typeof body?.databaseName === "string" ? body.databaseName : "",
          defaultDatabaseIdentifier,
        )
      : "0";
  const databaseUser =
    databaseKind === "postgres"
      ? databaseIdentifier(
          typeof body?.databaseUser === "string" ? body.databaseUser : "",
          "postgres",
        )
      : null;
  const databasePlainPassword =
    typeof body?.databasePassword === "string" && body.databasePassword
      ? body.databasePassword
      : databasePassword();
  const databasePublicEnabled = body?.databasePublicEnabled === true;
  const requestedPublicPort =
    typeof body?.databasePublicPort === "number" ? body.databasePublicPort : null;
  const databasePublicPort = databasePublicEnabled
    ? (requestedPublicPort ?? databasePort(databaseKind))
    : null;
  const repositoryUrl =
    appKind === "database"
      ? ""
      : typeof body?.repositoryUrl === "string"
        ? body.repositoryUrl.trim()
        : "";
  const sourceType =
    appKind === "database"
      ? "image"
      : SOURCE_TYPES.includes(body?.sourceType as AppSourceType)
        ? (body?.sourceType as AppSourceType)
        : "repository";
  const imageRef =
    appKind === "database"
      ? databaseImage(databaseKind, databaseVersion)
      : typeof body?.imageRef === "string"
        ? body.imageRef.trim()
        : "";
  const branch =
    typeof body?.branch === "string" && body.branch.trim() ? body.branch.trim() : "main";
  const port =
    appKind === "database"
      ? databasePort(databaseKind)
      : typeof body?.port === "number"
        ? body.port
        : 3000;
  const buildMode = BUILD_MODES.includes(body?.buildMode as AppBuildMode)
    ? (body?.buildMode as AppBuildMode)
    : "auto";
  const buildRunner =
    appKind === "database"
      ? "server"
      : BUILD_RUNNERS.includes(body?.buildRunner as AppBuildRunner)
        ? (body?.buildRunner as AppBuildRunner)
        : "depot";
  const serverIds = normalizeServerIds(body) ?? [];
  const serverId = serverIds[0] ?? null;
  const volumes = appKind === "database" ? [] : (normalizeVolumes(body?.volumes ?? []) ?? []);

  if (!(await ownedEnvironmentId(environmentId, organizationId))) {
    return c.json({ error: "Environment not found" }, 404);
  }
  if (!name) return c.json({ error: "name is required" }, 400);
  if (appKind === "database") {
    const versionError = validateDatabaseVersion(databaseVersion);
    if (versionError) return c.json({ error: versionError }, 400);
    const publicPortError = validatePublicPort(databasePublicPort);
    if (publicPortError) return c.json({ error: publicPortError }, 400);
    if (serverIds.length !== 1) {
      return c.json({ error: "Database apps require exactly one server" }, 400);
    }
  } else if (sourceType === "repository") {
    const repoError = validateRepositoryUrl(repositoryUrl);
    if (repoError) return c.json({ error: repoError }, 400);
  } else {
    const imageError = validateImageRef(imageRef);
    if (imageError) return c.json({ error: imageError }, 400);
  }
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return c.json({ error: "port must be a valid port" }, 400);
  }
  const volumeError = validateVolumes(volumes);
  if (volumeError) return c.json({ error: volumeError }, 400);
  if (!(await validateServersInOrg(serverIds, organizationId))) {
    return c.json({ error: "Server not found" }, 404);
  }

  const encryptedDatabasePassword =
    appKind === "database" ? await encryptSecret(databasePlainPassword) : null;

  const now = new Date();
  try {
    const created = await db.transaction(async (tx) => {
      const [row] = await tx
        .insert(app)
        .values({
          id: crypto.randomUUID(),
          environmentId,
          serverId,
          name,
          slug: slugify(name),
          repositoryUrl,
          imageRef: sourceType === "image" ? imageRef : null,
          sourceType,
          branch,
          port,
          buildMode,
          buildRunner,
          appKind,
          volumes: JSON.stringify(volumes),
          databaseKind: appKind === "database" ? databaseKind : null,
          databaseVersion: appKind === "database" ? databaseVersion : null,
          databaseName: appKind === "database" ? databaseName : null,
          databaseUser: appKind === "database" ? databaseUser : null,
          databasePassword: encryptedDatabasePassword,
          databasePublicEnabled,
          databasePublicPort,
          createdAt: now,
          updatedAt: now,
        })
        .returning();

      if (row && serverIds.length > 0) {
        await tx.insert(appServer).values(
          serverIds.map((selectedServerId) => ({
            appId: row.id,
            serverId: selectedServerId,
            createdAt: now,
          })),
        );
      }

      return row;
    });

    if (!created) return c.json({ error: "Failed to create app" }, 500);
    return c.json(toApp(created, serverIds), 201);
  } catch {
    return c.json({ error: "An app with that name already exists in this environment" }, 409);
  }
});

apps.patch("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as UpdateAppInput | null;
  const updates: Partial<AppRow> = { updatedAt: new Date() };

  if (typeof body?.name === "string" && body.name.trim()) {
    updates.name = body.name.trim();
    updates.slug = slugify(body.name);
  }
  if (typeof body?.repositoryUrl === "string") {
    const repoError = validateRepositoryUrl(body.repositoryUrl.trim());
    if (repoError) return c.json({ error: repoError }, 400);
    updates.repositoryUrl = body.repositoryUrl.trim();
  }
  if (SOURCE_TYPES.includes(body?.sourceType as AppSourceType)) {
    updates.sourceType = body?.sourceType as AppSourceType;
  }
  if (typeof body?.imageRef === "string" || body?.imageRef === null) {
    const imageRef = typeof body.imageRef === "string" ? body.imageRef.trim() : "";
    if ((updates.sourceType ?? existing.sourceType) === "image") {
      const imageError = validateImageRef(imageRef);
      if (imageError) return c.json({ error: imageError }, 400);
    }
    updates.imageRef = imageRef || null;
  }
  if (typeof body?.branch === "string" && body.branch.trim()) updates.branch = body.branch.trim();
  if (typeof body?.port === "number") {
    if (!Number.isInteger(body.port) || body.port < 1 || body.port > 65535) {
      return c.json({ error: "port must be a valid port" }, 400);
    }
    updates.port = body.port;
  }
  if (BUILD_MODES.includes(body?.buildMode as AppBuildMode)) {
    updates.buildMode = body?.buildMode as AppBuildMode;
  }
  if (BUILD_RUNNERS.includes(body?.buildRunner as AppBuildRunner)) {
    updates.buildRunner = body?.buildRunner as AppBuildRunner;
  }
  if (existing.appKind === "database") {
    const existingDatabaseKind = (existing.databaseKind ?? "postgres") as DatabaseKind;
    if (typeof body?.databaseVersion === "string" && body.databaseVersion.trim()) {
      const version = body.databaseVersion.trim();
      const versionError = validateDatabaseVersion(version);
      if (versionError) return c.json({ error: versionError }, 400);
      updates.databaseVersion = version;
      updates.imageRef = databaseImage(existingDatabaseKind, version);
    }
    if (typeof body?.databasePublicEnabled === "boolean") {
      updates.databasePublicEnabled = body.databasePublicEnabled;
      if (body.databasePublicEnabled && !(body && "databasePublicPort" in body)) {
        updates.databasePublicPort =
          existing.databasePublicPort ?? databasePort(existingDatabaseKind);
      }
      if (!body.databasePublicEnabled) {
        updates.databasePublicPort = null;
      }
    }
    if (body && "databasePublicPort" in body) {
      const publicPort =
        typeof body.databasePublicPort === "number" ? body.databasePublicPort : null;
      const publicPortError = validatePublicPort(publicPort);
      if (publicPortError) return c.json({ error: publicPortError }, 400);
      updates.databasePublicPort = publicPort;
    }
  }
  if (body && "volumes" in body) {
    if (existing.appKind === "database") {
      return c.json({ error: "Database volumes are managed by Basse" }, 400);
    }
    const volumes = normalizeVolumes(body.volumes);
    if (!volumes) return c.json({ error: "volumes must be an array" }, 400);
    const volumeError = validateVolumes(volumes);
    if (volumeError) return c.json({ error: volumeError }, 400);
    updates.volumes = JSON.stringify(volumes);
  }
  if ((updates.sourceType ?? existing.sourceType) === "repository") {
    const repoError = validateRepositoryUrl(updates.repositoryUrl ?? existing.repositoryUrl);
    if (repoError) return c.json({ error: repoError }, 400);
  }
  if ((updates.sourceType ?? existing.sourceType) === "image") {
    const imageError = validateImageRef(updates.imageRef ?? existing.imageRef ?? "");
    if (imageError) return c.json({ error: imageError }, 400);
  }
  const serverIds = normalizeServerIds(body);
  if (serverIds) {
    if (existing.appKind === "database" && serverIds.length !== 1) {
      return c.json({ error: "Database apps require exactly one server" }, 400);
    }
    if (!(await validateServersInOrg(serverIds, organizationId))) {
      return c.json({ error: "Server not found" }, 404);
    }
    updates.serverId = serverIds[0] ?? null;
  }

  const updated = await db.transaction(async (tx) => {
    const [row] = await tx.update(app).set(updates).where(eq(app.id, existing.id)).returning();
    if (serverIds) {
      await tx.delete(appServer).where(eq(appServer.appId, existing.id));
      if (serverIds.length > 0) {
        await tx.insert(appServer).values(
          serverIds.map((selectedServerId) => ({
            appId: existing.id,
            serverId: selectedServerId,
            createdAt: new Date(),
          })),
        );
      }
    }
    return row;
  });
  if (!updated) return c.json({ error: "Failed to update app" }, 500);
  const currentServerIds = serverIds ?? (await loadAppServerIds([updated.id])).get(updated.id);
  return c.json(toApp(updated, currentServerIds));
});

apps.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  await removeAppContainers([existing.id]);
  await db.delete(app).where(eq(app.id, existing.id));
  return c.body(null, 204);
});
