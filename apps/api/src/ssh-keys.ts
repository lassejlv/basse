import { db, sshKey } from "@basse/db";
import type { CreateSshKeyInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { encryptSecret } from "./crypto";
import { derivePublicKey } from "./server-keys";
import { resolveActiveWorkspace } from "./workspace";

type SshKeyRow = typeof sshKey.$inferSelect;

function sanitizeSshKey(row: SshKeyRow) {
  return {
    id: row.id,
    organizationId: row.organizationId,
    name: row.name,
    publicKey: row.publicKey,
    hasPrivateKey: Boolean(row.privateKey),
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

export const sshKeys = new Hono();

sshKeys.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(sshKey)
    .where(eq(sshKey.organizationId, organizationId))
    .orderBy(sshKey.createdAt);

  return c.json(rows.map(sanitizeSshKey));
});

sshKeys.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateSshKeyInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const privateKey = typeof body?.privateKey === "string" ? body.privateKey.trim() : "";

  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  if (!privateKey) {
    return c.json({ error: "privateKey is required" }, 400);
  }

  let publicKey: string;
  try {
    publicKey = await derivePublicKey(privateKey);
  } catch (error) {
    return c.json({ error: error instanceof Error ? error.message : "Invalid private key" }, 400);
  }

  const now = new Date();
  const [created] = await db
    .insert(sshKey)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      name,
      publicKey,
      privateKey: await encryptSecret(privateKey),
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) {
    return c.json({ error: "Failed to create SSH key" }, 500);
  }

  return c.json(sanitizeSshKey(created), 201);
});

sshKeys.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [deleted] = await db
    .delete(sshKey)
    .where(and(eq(sshKey.id, c.req.param("id")), eq(sshKey.organizationId, organizationId)))
    .returning({ id: sshKey.id });

  if (!deleted) {
    return c.json({ error: "SSH key not found" }, 404);
  }

  return c.body(null, 204);
});
