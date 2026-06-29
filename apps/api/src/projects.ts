import { db, member, project } from "@basse/db";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { auth } from "./auth";

/**
 * Resolves the caller's active workspace and confirms membership.
 * Returns the organization id, or a Response to short-circuit the request.
 */
async function resolveActiveWorkspace(headers: Headers): Promise<string | Response> {
  const session = await auth.api.getSession({ headers });

  if (!session) {
    return new Response("Unauthorized", { status: 401 });
  }

  const organizationId = session.session.activeOrganizationId;

  if (!organizationId) {
    return new Response("No active workspace", { status: 400 });
  }

  const [membership] = await db
    .select({ id: member.id })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.userId, session.user.id)))
    .limit(1);

  if (!membership) {
    return new Response("Forbidden", { status: 403 });
  }

  return organizationId;
}

function slugify(value: string) {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "");
}

export const projects = new Hono();

projects.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(project)
    .where(eq(project.organizationId, organizationId))
    .orderBy(project.createdAt);

  return c.json(rows);
});

projects.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as { name?: unknown } | null;
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
    const [created] = await db
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

    return c.json(created, 201);
  } catch {
    return c.json({ error: "A project with that name already exists in this workspace" }, 409);
  }
});
