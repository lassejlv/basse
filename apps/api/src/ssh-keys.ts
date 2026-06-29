import { db, sshKey } from "@basse/db";
import type { CreateSshKeyInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { resolveActiveWorkspace } from "./workspace";

const SSH_KEY_PREFIXES = [
  "ssh-rsa",
  "ssh-ed25519",
  "ssh-dss",
  "ecdsa-sha2-nistp256",
  "ecdsa-sha2-nistp384",
  "ecdsa-sha2-nistp521",
  "sk-ssh-ed25519@openssh.com",
  "sk-ecdsa-sha2-nistp256@openssh.com",
];

function isValidPublicKey(value: string): boolean {
  return SSH_KEY_PREFIXES.some((prefix) => value.startsWith(`${prefix} `));
}

export const sshKeys = new Hono();

sshKeys.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(sshKey)
    .where(eq(sshKey.organizationId, organizationId))
    .orderBy(sshKey.createdAt);

  return c.json(rows);
});

sshKeys.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateSshKeyInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const publicKey = typeof body?.publicKey === "string" ? body.publicKey.trim() : "";

  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  if (!isValidPublicKey(publicKey)) {
    return c.json({ error: "publicKey must be a valid SSH public key" }, 400);
  }

  const now = new Date();
  const [created] = await db
    .insert(sshKey)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      name,
      publicKey,
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  return c.json(created, 201);
});

sshKeys.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

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
