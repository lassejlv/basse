import { db, server } from "@basse/db";
import type { CreateServerInput, Server } from "@basse/shared";
import { and, eq, ne } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import { enqueueAction } from "./queue/queue";
import { connectionFromServer } from "./server-connection";
import { derivePublicKey, generateServerKeyPair } from "./server-keys";
import { probeReachable } from "./ssh";
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
  const providedPrivateKey =
    typeof body?.privateKey === "string" && body.privateKey.trim() ? body.privateKey.trim() : null;

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

  // Either reuse a pasted private key (deriving its public half) or generate a
  // new per-server keypair.
  let publicKey: string;
  let privateKey: string;

  if (providedPrivateKey) {
    try {
      publicKey = await derivePublicKey(providedPrivateKey);
      privateKey = providedPrivateKey;
    } catch (error) {
      return c.json(
        { error: error instanceof Error ? error.message : "Invalid private key" },
        400,
      );
    }
  } else {
    const keyPair = await generateServerKeyPair(id);
    publicKey = keyPair.publicKey;
    privateKey = keyPair.privateKey;
  }

  const encryptedPrivateKey = await encryptSecret(privateKey);
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
      sshPublicKey: publicKey,
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

servers.post("/:id/provision", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const id = c.req.param("id");

  // Atomically claim the row: only a server that is not already provisioning can
  // be (re-)provisioned. The conditional update is the concurrency lock.
  const claimed = await db
    .update(server)
    .set({ status: "provisioning", statusMessage: "Queued…", updatedAt: new Date() })
    .where(
      and(
        eq(server.id, id),
        eq(server.organizationId, organizationId),
        ne(server.status, "provisioning"),
      ),
    )
    .returning({ id: server.id });

  if (!claimed[0]) {
    // Either it does not exist/belong to the workspace, or it is already running.
    const [row] = await db
      .select({ id: server.id })
      .from(server)
      .where(and(eq(server.id, id), eq(server.organizationId, organizationId)))
      .limit(1);

    if (!row) {
      return c.json({ error: "Server not found" }, 404);
    }

    return c.json({ error: "Server is already provisioning" }, 409);
  }

  // Enqueue the durable job. The worker runs provisionServer (which owns all
  // status writes). If Redis is unreachable, revert the claim so the row isn't
  // stuck on "Queued…" and the user can retry.
  try {
    await enqueueAction("provision-server", id);
  } catch {
    await db
      .update(server)
      .set({
        status: "error",
        statusMessage: "Could not queue provisioning (queue unavailable). Retry.",
        updatedAt: new Date(),
      })
      .where(eq(server.id, id));

    return c.json({ error: "Could not queue provisioning" }, 503);
  }

  return c.body(null, 202);
});

servers.post("/:id/proxy/resync", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select({ id: server.id })
    .from(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }

  try {
    await enqueueAction("sync-domains", row.id);
  } catch {
    return c.json({ error: "Could not queue resync" }, 503);
  }

  return c.body(null, 202);
});

servers.post("/:id/check-connection", async (c) => {
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

  const connection = await connectionFromServer(row);
  const result = await probeReachable(connection);

  if (result.fingerprint && result.fingerprint !== row.hostKeyFingerprint) {
    await db
      .update(server)
      .set({ hostKeyFingerprint: result.fingerprint, updatedAt: new Date() })
      .where(eq(server.id, row.id));
  }

  return c.json({
    ok: result.ok,
    fingerprint: result.fingerprint,
    error: result.error,
  });
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
