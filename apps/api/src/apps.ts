import { app, db, environment, project, server } from "@basse/db";
import type { App, AppBuildMode, CreateAppInput, UpdateAppInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
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

function toApp(row: AppRow): App {
  return {
    id: row.id,
    environmentId: row.environmentId,
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

/** Verifies a server belongs to the active workspace. */
async function serverInOrg(serverId: string, organizationId: string): Promise<boolean> {
  const [row] = await db
    .select({ id: server.id })
    .from(server)
    .where(and(eq(server.id, serverId), eq(server.organizationId, organizationId)))
    .limit(1);
  return Boolean(row);
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

  return c.json(rows.map(toApp));
});

apps.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedApp(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  return c.json(toApp(row));
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
  const serverId = typeof body?.serverId === "string" && body.serverId ? body.serverId : null;

  if (!(await ownedEnvironmentId(environmentId, organizationId))) {
    return c.json({ error: "Environment not found" }, 404);
  }
  if (!name) return c.json({ error: "name is required" }, 400);
  const repoError = validateRepositoryUrl(repositoryUrl);
  if (repoError) return c.json({ error: repoError }, 400);
  if (!Number.isInteger(port) || port < 1 || port > 65535) {
    return c.json({ error: "port must be a valid port" }, 400);
  }
  if (serverId && !(await serverInOrg(serverId, organizationId))) {
    return c.json({ error: "Server not found" }, 404);
  }

  const now = new Date();
  try {
    const [created] = await db
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

    if (!created) return c.json({ error: "Failed to create app" }, 500);
    return c.json(toApp(created), 201);
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
  if (body && "serverId" in body) {
    if (body.serverId === null) {
      updates.serverId = null;
    } else if (typeof body.serverId === "string") {
      if (!(await serverInOrg(body.serverId, organizationId))) {
        return c.json({ error: "Server not found" }, 404);
      }
      updates.serverId = body.serverId;
    }
  }

  const [updated] = await db.update(app).set(updates).where(eq(app.id, existing.id)).returning();
  if (!updated) return c.json({ error: "Failed to update app" }, 500);
  return c.json(toApp(updated));
});

apps.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  await db.delete(app).where(eq(app.id, existing.id));
  return c.body(null, 204);
});
