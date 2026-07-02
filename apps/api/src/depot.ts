import { db, depotConnection } from "@basse/db";
import type { DepotConnection, SaveDepotConnectionInput } from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import { resolveActiveWorkspace } from "./workspace";

export const depot = new Hono();

depot.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [connection] = await db
    .select()
    .from(depotConnection)
    .where(eq(depotConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) {
    return c.json({ connected: false } satisfies DepotConnection);
  }

  let tokenHint = "";

  try {
    const token = await decryptSecret(connection.token);
    tokenHint = token.slice(-4);
  } catch {
    tokenHint = "";
  }

  return c.json({
    connected: true,
    projectId: connection.projectId,
    orgId: connection.orgId,
    tokenHint,
    updatedAt: connection.updatedAt.toISOString(),
  } satisfies DepotConnection);
});

depot.put("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<SaveDepotConnectionInput> | null;
  const token = typeof body?.token === "string" ? body.token.trim() : "";
  const projectId = typeof body?.projectId === "string" ? body.projectId.trim() : "";
  const orgId = typeof body?.orgId === "string" ? body.orgId.trim() : "";

  if (!token) {
    return c.json({ error: "token is required" }, 400);
  }

  if (!projectId) {
    return c.json({ error: "projectId is required" }, 400);
  }

  if (!orgId) {
    return c.json({ error: "orgId is required" }, 400);
  }

  const now = new Date();
  const encryptedToken = await encryptSecret(token);

  await db
    .insert(depotConnection)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      token: encryptedToken,
      projectId,
      orgId,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: depotConnection.organizationId,
      set: { token: encryptedToken, projectId, orgId, updatedAt: now },
    });

  return c.json({
    connected: true,
    projectId,
    orgId,
    tokenHint: token.slice(-4),
    updatedAt: now.toISOString(),
  } satisfies DepotConnection);
});

depot.delete("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  await db.delete(depotConnection).where(eq(depotConnection.organizationId, organizationId));

  return c.body(null, 204);
});
