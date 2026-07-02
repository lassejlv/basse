import { app, db, environment, project } from "@basse/db";
import type { CreateProjectInput } from "@basse/shared";
import { and, eq, inArray, sql } from "drizzle-orm";
import { Hono } from "hono";
import { removeAppContainers } from "./app-cleanup";
import { resolveActiveWorkspace } from "./workspace";

function slugify(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export const projects = new Hono();

projects.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(project)
    .where(eq(project.organizationId, organizationId))
    .orderBy(project.createdAt);

  const projectIds = rows.map((row) => row.id);
  const environmentCount = new Map<string, number>();
  const appCount = new Map<string, number>();

  if (projectIds.length > 0) {
    const envRows = await db
      .select({ projectId: environment.projectId, count: sql<number>`count(*)::int` })
      .from(environment)
      .where(inArray(environment.projectId, projectIds))
      .groupBy(environment.projectId);
    for (const row of envRows) environmentCount.set(row.projectId, row.count);

    const appRows = await db
      .select({ projectId: environment.projectId, count: sql<number>`count(*)::int` })
      .from(app)
      .innerJoin(environment, eq(app.environmentId, environment.id))
      .where(inArray(environment.projectId, projectIds))
      .groupBy(environment.projectId);
    for (const row of appRows) appCount.set(row.projectId, row.count);
  }

  return c.json(
    rows.map((row) => ({
      ...row,
      environmentCount: environmentCount.get(row.id) ?? 0,
      appCount: appCount.get(row.id) ?? 0,
    })),
  );
});

projects.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select()
    .from(project)
    .where(and(eq(project.id, c.req.param("id")), eq(project.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Project not found" }, 404);
  }

  return c.json(row);
});

projects.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateProjectInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";

  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  const slug = slugify(name);

  if (!slug) {
    return c.json({ error: "name must contain alphanumeric characters" }, 400);
  }

  const now = new Date();

  try {
    // Create the project and its default "production" environment atomically, so
    // the "every project has >= 1 environment" invariant always holds.
    const created = await db.transaction(async (tx) => {
      const [proj] = await tx
        .insert(project)
        .values({
          id: crypto.randomUUID(),
          organizationId,
          name,
          slug,
          createdAt: now,
          updatedAt: now,
        })
        .returning();

      if (!proj) {
        throw new Error("project insert failed");
      }

      await tx.insert(environment).values({
        id: crypto.randomUUID(),
        projectId: proj.id,
        name: "Production",
        slug: "production",
        isDefault: true,
        createdAt: now,
        updatedAt: now,
      });

      return proj;
    });

    return c.json(created, 201);
  } catch {
    return c.json({ error: "A project with that name already exists in this workspace" }, 409);
  }
});

projects.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [existing] = await db
    .select({ id: project.id })
    .from(project)
    .where(and(eq(project.id, c.req.param("id")), eq(project.organizationId, organizationId)))
    .limit(1);

  if (!existing) {
    return c.json({ error: "Project not found" }, 404);
  }

  const apps = await db
    .select({ id: app.id })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .where(eq(environment.projectId, existing.id));

  await removeAppContainers(apps.map((row) => row.id));
  await db.delete(project).where(eq(project.id, existing.id));
  return c.body(null, 204);
});
