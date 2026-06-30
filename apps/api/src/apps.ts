import { app, appServer, db, environment, project, server } from "@basse/db";
import type {
  App,
  AppBuildMode,
  AppConsoleResult,
  AppMetrics,
  CreateAppInput,
  UpdateAppInput,
} from "@basse/shared";
import { and, eq, inArray } from "drizzle-orm";
import { Hono } from "hono";
import { execAppCommand, getAppMetrics as getAgentAppMetrics } from "./agent-client";
import { decryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";
import { resolveActiveWorkspace } from "./workspace";

type AppRow = typeof app.$inferSelect;

const BUILD_MODES: AppBuildMode[] = ["auto", "dockerfile", "railpack"];

function slugify(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
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

function toApp(row: AppRow, serverIds: string[] = row.serverId ? [row.serverId] : []): App {
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
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
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

function normalizeServerIds(body: Partial<CreateAppInput | UpdateAppInput> | null): string[] | null {
  if (Array.isArray(body?.serverIds)) {
    return [...new Set(body.serverIds.filter((id): id is string => typeof id === "string" && id.length > 0))];
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

  const serverIds = await loadAppServerIds(rows.map((row) => row.id));
  return c.json(rows.map((row) => toApp(row, serverIds.get(row.id))));
});

apps.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedApp(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const serverIds = await loadAppServerIds([row.id]);
  return c.json(toApp(row, serverIds.get(row.id)));
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

apps.post("/:id/console", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as
    | { command?: unknown; serverId?: unknown }
    | null;
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

apps.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateAppInput> | null;
  const environmentId = typeof body?.environmentId === "string" ? body.environmentId : "";
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const repositoryUrl = typeof body?.repositoryUrl === "string" ? body.repositoryUrl.trim() : "";
  const branch = typeof body?.branch === "string" && body.branch.trim() ? body.branch.trim() : "main";
  const port = typeof body?.port === "number" ? body.port : 3000;
  const buildMode = BUILD_MODES.includes(body?.buildMode as AppBuildMode)
    ? (body?.buildMode as AppBuildMode)
    : "auto";
  const serverIds = normalizeServerIds(body) ?? [];
  const serverId = serverIds[0] ?? null;

  if (!(await ownedEnvironmentId(environmentId, organizationId))) {
    return c.json({ error: "Environment not found" }, 404);
  }
  if (!name) return c.json({ error: "name is required" }, 400);
  const repoError = validateRepositoryUrl(repositoryUrl);
  if (repoError) return c.json({ error: repoError }, 400);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return c.json({ error: "port must be a valid port" }, 400);
  }
  if (!(await validateServersInOrg(serverIds, organizationId))) {
    return c.json({ error: "Server not found" }, 404);
  }

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
          branch,
          port,
          buildMode,
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
  const serverIds = normalizeServerIds(body);
  if (serverIds) {
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

  await db.delete(app).where(eq(app.id, existing.id));
  return c.body(null, 204);
});
