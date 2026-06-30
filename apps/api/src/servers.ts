import { db, server } from "@basse/db";
import type { CreateServerInput, Server } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import { generateServerKeyPair } from "./server-keys";
import { resolveActiveWorkspace } from "./workspace";

type ServerRow = typeof server.$inferSelect;

/**
 * Maps a DB row to the client-facing DTO. Never exposes the private key or the
 * raw agent token — only the public key and a last-4 token hint (depot pattern).
 */
async function sanitizeServer(row: ServerRow): Promise<Server> {
  let agentTokenHint: string | undefined;

  if (row.agentToken) {
    try {
      const token = await decryptSecret(row.agentToken);
      agentTokenHint = token.slice(-4);
    } catch {
      agentTokenHint = undefined;
    }
  }

  return {
    id: row.id,
    organizationId: row.organizationId,
    name: row.name,
    sshHost: row.sshHost,
    sshPort: row.sshPort,
    sshUser: row.sshUser,
    sshPublicKey: row.sshPublicKey,
    agentUrl: row.agentUrl,
    status: row.status,
    statusMessage: row.statusMessage,
    hostKeyFingerprint: row.hostKeyFingerprint,
    agentTokenHint,
    lastSeenAt: row.lastSeenAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

export const servers = new Hono();

servers.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(server)
    .where(eq(server.organizationId, organizationId))
    .orderBy(server.createdAt);

  return c.json(await Promise.all(rows.map(sanitizeServer)));
});

servers.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select()
    .from(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }

  return c.json(await sanitizeServer(row));
});

servers.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateServerInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const sshHost = typeof body?.sshHost === "string" ? body.sshHost.trim() : "";
  const sshPort = typeof body?.sshPort === "number" ? body.sshPort : 22;
  const sshUser = typeof body?.sshUser === "string" && body.sshUser.trim() ? body.sshUser.trim() : "root";

  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  if (!sshHost) {
    return c.json({ error: "sshHost is required" }, 400);
  }

  if (!Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) {
    return c.json({ error: "sshPort must be a valid port" }, 400);
  }

  const id = crypto.randomUUID();
  const keyPair = await generateServerKeyPair(id);
  const encryptedPrivateKey = await encryptSecret(keyPair.privateKey);
  const now = new Date();

  const [created] = await db
    .insert(server)
    .values({
      id,
      organizationId,
      name,
      sshHost,
      sshPort,
      sshUser,
      sshPublicKey: keyPair.publicKey,
      sshPrivateKey: encryptedPrivateKey,
      status: "pending",
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) {
    return c.json({ error: "Failed to create server" }, 500);
  }

  return c.json(await sanitizeServer(created), 201);
});

servers.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [deleted] = await db
    .delete(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .returning({ id: server.id });

  if (!deleted) {
    return c.json({ error: "Server not found" }, 404);
  }

  return c.body(null, 204);
});
