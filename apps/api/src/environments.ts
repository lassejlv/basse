import { db, environment, project } from "@basse/db";
import type { CreateEnvironmentInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { resolveActiveWorkspace } from "./workspace";

function slugify(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

/** Confirms a project exists and belongs to the active workspace. */
async function ownedProjectId(projectId: string, organizationId: string): Promise<string | null> {
  const [row] = await db
    .select({ id: project.id })
    .from(project)
    .where(and(eq(project.id, projectId), eq(project.organizationId, organizationId)))
    .limit(1);
  return row?.id ?? null;
}

export const environments = new Hono();

environments.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.query("projectId");
  if (!projectId || !(await ownedProjectId(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const rows = await db
    .select()
    .from(environment)
    .where(eq(environment.projectId, projectId))
    .orderBy(environment.createdAt);

  return c.json(rows);
});

environments.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateEnvironmentInput> | null;
  const projectId = typeof body?.projectId === "string" ? body.projectId : "";
  const name = typeof body?.name === "string" ? body.name.trim() : "";

  if (!(await ownedProjectId(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }
  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  const slug = slugify(name);
  if (!slug) {
    return c.json({ error: "name must contain alphanumeric characters" }, 400);
  }

  const now = new Date();
  try {
    const [created] = await db
      .insert(environment)
      .values({
        id: crypto.randomUUID(),
        projectId,
        name,
        slug,
        isDefault: false,
        createdAt: now,
        updatedAt: now,
      })
      .returning();

    return c.json(created, 201);
  } catch {
    return c.json({ error: "An environment with that name already exists" }, 409);
  }
});
