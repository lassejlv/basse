import { app, appServer, db, deployment, environment, envVar, project, server } from "@basse/db";
import type {
  App,
  AppBuildMode,
  AppBuildRunner,
  AppKind,
  AppConsoleResult,
  DatabaseConnectionInfo,
  DatabaseKind,
  ImportDockerContainerInput,
  ImportableDockerContainer,
  AppLogs,
  AppMetrics,
  AppSourceType,
  AppVolume,
  CreateAppInput,
  DeploymentStatus,
  UpdateAppInput,
} from "@basse/shared";
import { and, desc, eq, inArray, ne } from "drizzle-orm";
import { Hono } from "hono";
import { removeAppContainers } from "../deploy/app-cleanup";
import {
  execAppCommand,
  getAppLogs as getAgentAppLogs,
  getAppMetrics as getAgentAppMetrics,
  importContainer,
  listImportableContainers,
  type AgentConnection,
} from "../infra/agent-client";
import {
  DEFAULT_BUILD_ROOT_DIRECTORY,
  DEFAULT_DOCKERFILE_PATH,
  normalizeBuildRootDirectory,
  normalizeDockerfilePath,
} from "../deploy/build-paths";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { createNeonProject, deleteNeonProject, getNeonApiKey } from "../integrations/neon";
import { connectionFromServer } from "../infra/server-connection";
import { enqueueAction } from "../queue/queue";
import { runScript, type SshConnection } from "../infra/ssh";
import { resolveActiveWorkspace } from "../lib/workspace";

type AppRow = typeof app.$inferSelect;

const BUILD_MODES: AppBuildMode[] = ["auto", "dockerfile", "railpack"];
const BUILD_RUNNERS: AppBuildRunner[] = ["depot", "server"];
const APP_KINDS: AppKind[] = ["service", "database", "neon"];
const SOURCE_TYPES: AppSourceType[] = ["repository", "image"];
const DATABASE_KINDS: DatabaseKind[] = ["postgres", "redis"];
const DEFAULT_POSTGRES_VERSION = "18";
const DEFAULT_REDIS_VERSION = "8";
const POSTGRES_PORT = 5432;
const REDIS_PORT = 6379;

export function slugify(value: string) {
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
  return `redis://default:${password}@${input.host}:${input.port}/${database}`;
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

function normalizeWebhookUrl(value: string | null): string | null {
  if (value === null) return null;
  const trimmed = value.trim();
  return trimmed || null;
}

function validateWebhookUrl(value: string | null): string | null {
  if (value === null) return null;
  if (value.length > 2048) return "Webhook URL is too long";
  let parsed: URL;
  try {
    parsed = new URL(value);
  } catch {
    return "Webhook URL must be a valid URL";
  }
  if (parsed.protocol !== "https:" && parsed.protocol !== "http:") {
    return "Webhook URL must use http or https";
  }
  return null;
}

function normalizeBuildPathInput(
  value: string | undefined,
  normalize: (input?: string | null) => string,
): { ok: true; value: string } | { ok: false; error: string } {
  try {
    return { ok: true, value: normalize(value) };
  } catch (error) {
    return { ok: false, error: error instanceof Error ? error.message : "Invalid build path" };
  }
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

function importedEnvVars(values: string[]): { key: string; value: string }[] {
  const vars = new Map<string, string>();
  for (const value of values) {
    const equals = value.indexOf("=");
    if (equals <= 0) continue;
    const key = value.slice(0, equals);
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) continue;
    vars.set(key, value.slice(equals + 1));
  }
  return [...vars].map(([key, value]) => ({ key, value }));
}

function importedVolumes(
  mounts: { source: string; destination: string; readOnly: boolean }[],
): AppVolume[] {
  return mounts
    .filter((mount) => mount.source.startsWith("/") && mount.destination.startsWith("/"))
    .map((mount) => ({
      hostPath: mount.source,
      containerPath: mount.destination,
      readOnly: mount.readOnly,
    }));
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

function isSshConnection(connection: AgentConnection): connection is SshConnection {
  return !("mode" in connection);
}

export function toApp(
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

  const neon =
    row.appKind === "neon" && row.neonProjectId
      ? {
          projectId: row.neonProjectId,
          region: row.neonRegion ?? "",
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
    buildRootDirectory: row.buildRootDirectory,
    dockerfilePath: row.dockerfilePath,
    buildRunner: row.buildRunner,
    autoRedeployEnabled: row.autoRedeployEnabled,
    appKind: row.appKind,
    sourceType: row.sourceType,
    imageRef: row.imageRef,
    volumes: parseVolumes(row.volumes),
    resourceLimits: {
      cpuMillicores: row.cpuLimitMillicores,
      memoryBytes: row.memoryLimitBytes,
    },
    healthCheck: {
      enabled: row.healthCheckEnabled,
      path: row.healthCheckPath,
      expectedStatus: row.healthCheckStatus,
      timeoutSeconds: row.healthCheckTimeoutSeconds,
      intervalSeconds: row.healthCheckIntervalSeconds,
    },
    deployNotifications: {
      webhookUrl: row.deployWebhookUrl,
      notifyOnSuccess: row.deployNotifySuccess,
      notifyOnFailure: row.deployNotifyFailure,
    },
    database,
    neon,
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

export async function loadAppServerIds(appIds: string[]): Promise<Map<string, string[]>> {
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

function normalizeCpuLimit(value: unknown): number | null | undefined {
  if (value === null) return null;
  if (typeof value === "undefined") return undefined;
  if (typeof value !== "number" || !Number.isInteger(value)) return undefined;
  return value;
}

function normalizeMemoryLimit(value: unknown): number | null | undefined {
  if (value === null) return null;
  if (typeof value === "undefined") return undefined;
  if (typeof value !== "number" || !Number.isInteger(value)) return undefined;
  return value;
}

function validateCpuLimit(value: number | null | undefined): string | null {
  if (value == null) return null;
  if (value < 50 || value > 256_000) {
    return "CPU limit must be between 0.05 and 256 CPU cores";
  }
  return null;
}

function validateMemoryLimit(value: number | null | undefined): string | null {
  if (value == null) return null;
  if (value < 16 * 1024 * 1024 || value > Number.MAX_SAFE_INTEGER) {
    return "Memory limit must be at least 16 MB";
  }
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

export async function validateServersInOrg(
  serverIds: string[],
  organizationId: string,
): Promise<boolean> {
  if (serverIds.length === 0) return true;
  const rows = await db
    .select({ id: server.id })
    .from(server)
    .where(and(inArray(server.id, serverIds), eq(server.organizationId, organizationId)));
  return rows.length === serverIds.length;
}

async function ownedServer(serverId: string, organizationId: string) {
  const [row] = await db
    .select()
    .from(server)
    .where(and(eq(server.id, serverId), eq(server.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
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

export async function requireAgentTarget(appId: string, requestedServerId?: string) {
  const resolved = await resolveAttachedServer(appId, requestedServerId);
  if (!resolved.server) return resolved;
  if (resolved.server.status !== "active" || !resolved.server.agentToken) {
    return { error: "Target server is not active" };
  }
  const connection = await connectionFromServer(resolved.server);
  const token = await decryptSecret(resolved.server.agentToken);
  return { server: resolved.server, connection, token };
}

export type BuildAppUpdatesResult =
  | { ok: true; updates: Partial<AppRow>; serverIds: string[] | null }
  | { ok: false; error: string; status: 400 | 404 };

/**
 * Validates and normalizes a partial app-config patch against the existing row,
 * returning the `Partial<AppRow>` to write (without `updatedAt`) plus the
 * resolved server set (or null when servers were not part of the patch). Shared
 * by PATCH /:id (immediate write) and the staged-changes router (stage + apply)
 * so both run identical validation and field derivation. Never throws.
 */
export async function buildAppUpdates(
  existing: AppRow,
  body: UpdateAppInput | null,
  organizationId: string,
): Promise<BuildAppUpdatesResult> {
  const updates: Partial<AppRow> = {};

  if (typeof body?.name === "string" && body.name.trim()) {
    updates.name = body.name.trim();
    updates.slug = slugify(body.name);
  }
  const nextSourceType = SOURCE_TYPES.includes(body?.sourceType as AppSourceType)
    ? (body?.sourceType as AppSourceType)
    : existing.sourceType;
  if (SOURCE_TYPES.includes(body?.sourceType as AppSourceType)) {
    updates.sourceType = body?.sourceType as AppSourceType;
  }
  if (typeof body?.repositoryUrl === "string" && nextSourceType === "repository") {
    const repoError = validateRepositoryUrl(body.repositoryUrl.trim());
    if (repoError) return { ok: false, error: repoError, status: 400 };
    updates.repositoryUrl = body.repositoryUrl.trim();
  }
  if (typeof body?.imageRef === "string" || body?.imageRef === null) {
    const imageRef = typeof body.imageRef === "string" ? body.imageRef.trim() : "";
    if (nextSourceType === "image") {
      const imageError = validateImageRef(imageRef);
      if (imageError) return { ok: false, error: imageError, status: 400 };
    }
    updates.imageRef = imageRef || null;
  }
  if (typeof body?.branch === "string" && body.branch.trim()) updates.branch = body.branch.trim();
  if (typeof body?.port === "number") {
    if (!Number.isInteger(body.port) || body.port < 1 || body.port > 65535) {
      return { ok: false, error: "port must be a valid port", status: 400 };
    }
    updates.port = body.port;
  }
  if (BUILD_MODES.includes(body?.buildMode as AppBuildMode)) {
    updates.buildMode = body?.buildMode as AppBuildMode;
  }
  if (typeof body?.buildRootDirectory === "string") {
    const normalized = normalizeBuildPathInput(
      body.buildRootDirectory,
      normalizeBuildRootDirectory,
    );
    if (!normalized.ok) return { ok: false, error: normalized.error, status: 400 };
    updates.buildRootDirectory = normalized.value;
  }
  if (typeof body?.dockerfilePath === "string") {
    const normalized = normalizeBuildPathInput(body.dockerfilePath, normalizeDockerfilePath);
    if (!normalized.ok) return { ok: false, error: normalized.error, status: 400 };
    updates.dockerfilePath = normalized.value;
  }
  if (BUILD_RUNNERS.includes(body?.buildRunner as AppBuildRunner)) {
    updates.buildRunner = body?.buildRunner as AppBuildRunner;
  }
  if (typeof body?.autoRedeployEnabled === "boolean") {
    updates.autoRedeployEnabled = body.autoRedeployEnabled;
  }
  if (typeof body?.healthCheckEnabled === "boolean") {
    updates.healthCheckEnabled = body.healthCheckEnabled;
  }
  if (typeof body?.healthCheckPath === "string") {
    const path = body.healthCheckPath.trim() || "/";
    if (!path.startsWith("/") || path.length > 500 || /\s/.test(path)) {
      return {
        ok: false,
        error: "Health check path must start with / and contain no spaces",
        status: 400,
      };
    }
    updates.healthCheckPath = path;
  }
  if (typeof body?.healthCheckStatus === "number") {
    if (
      !Number.isInteger(body.healthCheckStatus) ||
      body.healthCheckStatus < 100 ||
      body.healthCheckStatus > 599
    ) {
      return { ok: false, error: "Expected status must be 100-599", status: 400 };
    }
    updates.healthCheckStatus = body.healthCheckStatus;
  }
  if (typeof body?.healthCheckTimeoutSeconds === "number") {
    if (
      !Number.isInteger(body.healthCheckTimeoutSeconds) ||
      body.healthCheckTimeoutSeconds < 1 ||
      body.healthCheckTimeoutSeconds > 60
    ) {
      return { ok: false, error: "Health check timeout must be 1-60 seconds", status: 400 };
    }
    updates.healthCheckTimeoutSeconds = body.healthCheckTimeoutSeconds;
  }
  if (typeof body?.healthCheckIntervalSeconds === "number") {
    if (
      !Number.isInteger(body.healthCheckIntervalSeconds) ||
      body.healthCheckIntervalSeconds < 10 ||
      body.healthCheckIntervalSeconds > 3600
    ) {
      return { ok: false, error: "Health check interval must be 10-3600 seconds", status: 400 };
    }
    updates.healthCheckIntervalSeconds = body.healthCheckIntervalSeconds;
  }
  if (body && "deployWebhookUrl" in body) {
    const webhookUrl =
      typeof body.deployWebhookUrl === "string" || body.deployWebhookUrl === null
        ? normalizeWebhookUrl(body.deployWebhookUrl)
        : undefined;
    if (typeof webhookUrl === "undefined") {
      return { ok: false, error: "Webhook URL must be a string or null", status: 400 };
    }
    const webhookError = validateWebhookUrl(webhookUrl);
    if (webhookError) return { ok: false, error: webhookError, status: 400 };
    updates.deployWebhookUrl = webhookUrl;
  }
  if (typeof body?.deployNotifySuccess === "boolean") {
    updates.deployNotifySuccess = body.deployNotifySuccess;
  }
  if (typeof body?.deployNotifyFailure === "boolean") {
    updates.deployNotifyFailure = body.deployNotifyFailure;
  }
  if (existing.appKind === "database") {
    const existingDatabaseKind = (existing.databaseKind ?? "postgres") as DatabaseKind;
    if (typeof body?.databaseVersion === "string" && body.databaseVersion.trim()) {
      const version = body.databaseVersion.trim();
      const versionError = validateDatabaseVersion(version);
      if (versionError) return { ok: false, error: versionError, status: 400 };
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
      if (publicPortError) return { ok: false, error: publicPortError, status: 400 };
      updates.databasePublicPort = publicPort;
    }
  }
  if (body && "volumes" in body) {
    if (existing.appKind === "database") {
      return { ok: false, error: "Database volumes are managed by Basse", status: 400 };
    }
    const volumes = normalizeVolumes(body.volumes);
    if (!volumes) return { ok: false, error: "volumes must be an array", status: 400 };
    const volumeError = validateVolumes(volumes);
    if (volumeError) return { ok: false, error: volumeError, status: 400 };
    updates.volumes = JSON.stringify(volumes);
  }
  if (body && "cpuLimitMillicores" in body) {
    const cpuLimit = normalizeCpuLimit(body.cpuLimitMillicores);
    if (typeof cpuLimit === "undefined") {
      return {
        ok: false,
        error: "CPU limit must be a whole number of millicores or null",
        status: 400,
      };
    }
    const cpuLimitError = validateCpuLimit(cpuLimit);
    if (cpuLimitError) return { ok: false, error: cpuLimitError, status: 400 };
    updates.cpuLimitMillicores = cpuLimit;
  }
  if (body && "memoryLimitBytes" in body) {
    const memoryLimit = normalizeMemoryLimit(body.memoryLimitBytes);
    if (typeof memoryLimit === "undefined") {
      return { ok: false, error: "Memory limit must be bytes or null", status: 400 };
    }
    const memoryLimitError = validateMemoryLimit(memoryLimit);
    if (memoryLimitError) return { ok: false, error: memoryLimitError, status: 400 };
    updates.memoryLimitBytes = memoryLimit;
  }
  if (existing.appKind !== "neon" && (updates.sourceType ?? existing.sourceType) === "repository") {
    const repoError = validateRepositoryUrl(updates.repositoryUrl ?? existing.repositoryUrl);
    if (repoError) return { ok: false, error: repoError, status: 400 };
  }
  if (existing.appKind !== "neon" && (updates.sourceType ?? existing.sourceType) === "image") {
    const imageError = validateImageRef(updates.imageRef ?? existing.imageRef ?? "");
    if (imageError) return { ok: false, error: imageError, status: 400 };
  }
  const serverIds = normalizeServerIds(body);
  if (serverIds) {
    if (existing.appKind === "database" && serverIds.length !== 1) {
      return { ok: false, error: "Database apps require exactly one server", status: 400 };
    }
    if (existing.appKind === "neon" && serverIds.length > 0) {
      return { ok: false, error: "Neon databases run on Neon, not on your servers", status: 400 };
    }
    if (!(await validateServersInOrg(serverIds, organizationId))) {
      return { ok: false, error: "Server not found", status: 404 };
    }
    updates.serverId = serverIds[0] ?? null;
  }

  return { ok: true, updates, serverIds };
}

export const apps = new Hono();

apps.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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

apps.get("/importable-containers", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const serverId = c.req.query("serverId") ?? "";
  const target = await ownedServer(serverId, organizationId);
  if (!target) return c.json({ error: "Server not found" }, 404);
  if (target.status !== "active" || !target.agentToken) {
    return c.json({ error: "Server is not active" }, 400);
  }

  const connection = await connectionFromServer(target);
  const token = await decryptSecret(target.agentToken);
  const containers = await listImportableContainers(connection, token);
  return c.json(
    containers.map(
      (container): ImportableDockerContainer => ({
        id: container.id,
        name: container.name,
        image: container.image,
        imageId: container.imageId,
        state: container.state,
        status: container.status,
        running: container.running,
        ports: container.ports,
      }),
    ),
  );
});

apps.post("/import-container", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<ImportDockerContainerInput> | null;
  const environmentId = typeof body?.environmentId === "string" ? body.environmentId : "";
  const serverId = typeof body?.serverId === "string" ? body.serverId : "";
  const containerId = typeof body?.containerId === "string" ? body.containerId : "";
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const port = typeof body?.port === "number" ? body.port : 0;

  if (!(await ownedEnvironmentId(environmentId, organizationId))) {
    return c.json({ error: "Environment not found" }, 404);
  }
  const target = await ownedServer(serverId, organizationId);
  if (!target) return c.json({ error: "Server not found" }, 404);
  if (target.status !== "active" || !target.agentToken) {
    return c.json({ error: "Server is not active" }, 400);
  }
  if (!containerId) return c.json({ error: "containerId is required" }, 400);
  if (!name) return c.json({ error: "name is required" }, 400);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return c.json({ error: "port must be a valid port" }, 400);
  }

  const slug = slugify(name);
  const [existing] = await db
    .select({ id: app.id })
    .from(app)
    .where(and(eq(app.environmentId, environmentId), eq(app.slug, slug)))
    .limit(1);
  if (existing) {
    return c.json({ error: "An app with that name already exists in this environment" }, 409);
  }

  const connection = await connectionFromServer(target);
  const token = await decryptSecret(target.agentToken);
  const candidates = await listImportableContainers(connection, token);
  const candidate = candidates.find((container) => container.id === containerId);
  if (!candidate) return c.json({ error: "Container not found or already managed" }, 404);
  if (!candidate.running) return c.json({ error: "Only running containers can be imported" }, 400);
  const imageError = validateImageRef(candidate.image);
  if (imageError) return c.json({ error: imageError }, 400);

  const appId = crypto.randomUUID();
  const imported = await importContainer(connection, token, { appId, containerId });
  const volumes = importedVolumes(imported.mounts).slice(0, 20);
  const imageRef =
    imported.image && !validateImageRef(imported.image) ? imported.image : candidate.image;
  const importedVars = importedEnvVars(imported.env);
  const now = new Date();

  const created = await db.transaction(async (tx) => {
    const [row] = await tx
      .insert(app)
      .values({
        id: appId,
        environmentId,
        serverId,
        name,
        slug,
        repositoryUrl: "",
        imageRef,
        sourceType: "image",
        branch: "main",
        port,
        buildMode: "auto",
        buildRunner: "server",
        appKind: "service",
        volumes: JSON.stringify(volumes),
        cpuLimitMillicores: null,
        memoryLimitBytes: null,
        databaseKind: null,
        databaseVersion: null,
        databaseName: null,
        databaseUser: null,
        databasePassword: null,
        databasePublicEnabled: false,
        databasePublicPort: null,
        createdAt: now,
        updatedAt: now,
      })
      .returning();

    await tx.insert(appServer).values({ appId, serverId, createdAt: now });
    if (importedVars.length > 0) {
      await tx.insert(envVar).values(
        await Promise.all(
          importedVars.map(async (entry) => ({
            id: crypto.randomUUID(),
            appId,
            key: entry.key,
            value: await encryptSecret(entry.value),
            createdAt: now,
            updatedAt: now,
          })),
        ),
      );
    }
    await tx.insert(deployment).values({
      id: crypto.randomUUID(),
      appId,
      status: imported.running ? "healthy" : "stopped",
      imageRef,
      logs: `Imported Docker container ${candidate.name} as basse-app-${appId}.\n`,
      createdAt: now,
      updatedAt: now,
    });

    return row;
  });

  if (!created) return c.json({ error: "Failed to import container" }, 500);
  return c.json(toApp(created, [serverId], imported.running ? "healthy" : "stopped"), 201);
});

apps.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  if (!isSshConnection(target.connection!)) {
    return c.json({ error: "Stopping apps on outbound servers is not supported yet" }, 400);
  }

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
    .where(and(eq(deployment.appId, appId), inArray(deployment.status, ["healthy", "crashed"])));

  return c.json({ ok: true });
});

apps.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateAppInput> | null;
  const environmentId = typeof body?.environmentId === "string" ? body.environmentId : "";
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const appKind = APP_KINDS.includes(body?.appKind as AppKind)
    ? (body?.appKind as AppKind)
    : "service";

  // Neon databases are provisioned on Neon's platform, not deployed to a
  // server: create the Neon project, store the (encrypted) connection string,
  // expose it as DATABASE_URL, and record a synthetic healthy deployment.
  if (appKind === "neon") {
    const neonRegion = typeof body?.neonRegion === "string" ? body.neonRegion.trim() : "";
    if (!(await ownedEnvironmentId(environmentId, organizationId))) {
      return c.json({ error: "Environment not found" }, 404);
    }
    if (!name) return c.json({ error: "name is required" }, 400);
    if (!neonRegion) return c.json({ error: "neonRegion is required" }, 400);

    const apiKey = await getNeonApiKey(organizationId);
    if (!apiKey) {
      return c.json({ error: "Connect a Neon API key in Secrets first" }, 400);
    }

    const slug = slugify(name);
    const [duplicate] = await db
      .select({ id: app.id })
      .from(app)
      .where(and(eq(app.environmentId, environmentId), eq(app.slug, slug)))
      .limit(1);
    if (duplicate) {
      return c.json({ error: "An app with that name already exists in this environment" }, 409);
    }

    let provisioned: Awaited<ReturnType<typeof createNeonProject>>;
    try {
      provisioned = await createNeonProject(apiKey, { name, regionId: neonRegion });
    } catch (error) {
      return c.json(
        { error: error instanceof Error ? error.message : "Could not create the Neon project" },
        502,
      );
    }

    const now = new Date();
    try {
      const created = await db.transaction(async (tx) => {
        const [row] = await tx
          .insert(app)
          .values({
            id: crypto.randomUUID(),
            environmentId,
            serverId: null,
            name,
            slug,
            repositoryUrl: "",
            sourceType: "repository",
            branch: "main",
            port: POSTGRES_PORT,
            buildMode: "auto",
            buildRunner: "server",
            autoRedeployEnabled: false,
            appKind: "neon",
            volumes: "[]",
            neonProjectId: provisioned.projectId,
            neonRegion: provisioned.regionId,
            neonConnectionUri: await encryptSecret(provisioned.connectionUri),
            createdAt: now,
            updatedAt: now,
          })
          .returning();
        if (!row) return row;

        await tx.insert(envVar).values({
          id: crypto.randomUUID(),
          appId: row.id,
          key: "DATABASE_URL",
          value: await encryptSecret(provisioned.connectionUri),
          createdAt: now,
          updatedAt: now,
        });
        await tx.insert(deployment).values({
          id: crypto.randomUUID(),
          appId: row.id,
          status: "healthy",
          logs: `Provisioned Neon project ${provisioned.projectId} in ${provisioned.regionId}.\n`,
          createdAt: now,
          updatedAt: now,
        });

        return row;
      });

      if (!created) {
        await deleteNeonProject(apiKey, provisioned.projectId).catch(() => {});
        return c.json({ error: "Failed to create app" }, 500);
      }
      return c.json(toApp(created, [], "healthy"), 201);
    } catch {
      // The row didn't land — don't leave an orphaned project on Neon.
      await deleteNeonProject(apiKey, provisioned.projectId).catch(() => {});
      return c.json({ error: "Failed to create app" }, 500);
    }
  }

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
  const normalizedBuildRootDirectory =
    appKind === "database"
      ? { ok: true as const, value: DEFAULT_BUILD_ROOT_DIRECTORY }
      : normalizeBuildPathInput(
          typeof body?.buildRootDirectory === "string" ? body.buildRootDirectory : undefined,
          normalizeBuildRootDirectory,
        );
  if (!normalizedBuildRootDirectory.ok) {
    return c.json({ error: normalizedBuildRootDirectory.error }, 400);
  }
  const normalizedDockerfilePath =
    appKind === "database"
      ? { ok: true as const, value: DEFAULT_DOCKERFILE_PATH }
      : normalizeBuildPathInput(
          typeof body?.dockerfilePath === "string" ? body.dockerfilePath : undefined,
          normalizeDockerfilePath,
        );
  if (!normalizedDockerfilePath.ok) {
    return c.json({ error: normalizedDockerfilePath.error }, 400);
  }
  const buildRunner =
    appKind === "database"
      ? "server"
      : BUILD_RUNNERS.includes(body?.buildRunner as AppBuildRunner)
        ? (body?.buildRunner as AppBuildRunner)
        : "depot";
  const autoRedeployEnabled =
    appKind === "database"
      ? false
      : typeof body?.autoRedeployEnabled === "boolean"
        ? body.autoRedeployEnabled
        : true;
  const cpuLimitMillicores =
    body && "cpuLimitMillicores" in body ? normalizeCpuLimit(body.cpuLimitMillicores) : null;
  if (typeof cpuLimitMillicores === "undefined") {
    return c.json({ error: "CPU limit must be a whole number of millicores or null" }, 400);
  }
  const memoryLimitBytes =
    body && "memoryLimitBytes" in body ? normalizeMemoryLimit(body.memoryLimitBytes) : null;
  if (typeof memoryLimitBytes === "undefined") {
    return c.json({ error: "Memory limit must be bytes or null" }, 400);
  }
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
  const cpuLimitError = validateCpuLimit(cpuLimitMillicores);
  if (cpuLimitError) return c.json({ error: cpuLimitError }, 400);
  const memoryLimitError = validateMemoryLimit(memoryLimitBytes);
  if (memoryLimitError) return c.json({ error: memoryLimitError }, 400);
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
          buildRootDirectory: normalizedBuildRootDirectory.value,
          dockerfilePath: normalizedDockerfilePath.value,
          buildRunner,
          autoRedeployEnabled,
          appKind,
          volumes: JSON.stringify(volumes),
          cpuLimitMillicores,
          memoryLimitBytes,
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as UpdateAppInput | null;
  const result = await buildAppUpdates(existing, body, organizationId);
  if (!result.ok) return c.json({ error: result.error }, result.status);

  if (result.updates.slug && result.updates.slug !== existing.slug) {
    const [duplicate] = await db
      .select({ id: app.id })
      .from(app)
      .where(
        and(
          eq(app.environmentId, existing.environmentId),
          eq(app.slug, result.updates.slug),
          ne(app.id, existing.id),
        ),
      )
      .limit(1);
    if (duplicate) {
      return c.json({ error: "An app with that name already exists in this environment" }, 409);
    }
  }

  const updates: Partial<AppRow> = { ...result.updates, updatedAt: new Date() };
  const serverIds = result.serverIds;

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
  if (serverIds) {
    await enqueueAction("sync-app-load-balancers", updated.id).catch((error) => {
      console.error("[load-balancer-resync]", updated.id, error);
    });
  }
  return c.json(toApp(updated, currentServerIds));
});

apps.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  if (existing.appKind === "neon" && existing.neonProjectId) {
    const apiKey = await getNeonApiKey(organizationId);
    if (apiKey) {
      await deleteNeonProject(apiKey, existing.neonProjectId).catch((error) => {
        console.error("[neon-delete]", existing.neonProjectId, error);
      });
    }
  }

  await removeAppContainers([existing.id]);
  await db.delete(app).where(eq(app.id, existing.id));
  return c.body(null, 204);
});
