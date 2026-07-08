import { db, hetznerConnection, server } from "@basse/db";
import type {
  CreateHetznerServerInput,
  HetznerConnection,
  HetznerLocation,
  HetznerServerType,
  SaveHetznerConnectionInput,
} from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import {
  createHetznerCloudServer,
  getHetznerToken,
  listHetznerLocations,
  listHetznerServerTypes,
  validateHetznerToken,
} from "../integrations/hetzner";
import { generateServerKeyPair } from "../infra/server-keys";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { resolveActiveWorkspace } from "../lib/workspace";
import { enqueueAction } from "../queue/queue";

export const hetzner = new Hono();

hetzner.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const [connection] = await db
    .select()
    .from(hetznerConnection)
    .where(eq(hetznerConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) {
    return c.json({ connected: false } satisfies HetznerConnection);
  }

  let tokenHint = "";
  try {
    const apiToken = await decryptSecret(connection.apiToken);
    tokenHint = apiToken.slice(-4);
  } catch {
    tokenHint = "";
  }

  return c.json({
    connected: true,
    tokenHint,
    updatedAt: connection.updatedAt.toISOString(),
  } satisfies HetznerConnection);
});

hetzner.put("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<SaveHetznerConnectionInput> | null;
  const apiToken = typeof body?.apiToken === "string" ? body.apiToken.trim() : "";
  if (!apiToken) {
    return c.json({ error: "apiToken is required" }, 400);
  }

  // Round-trip the token against the Hetzner Cloud API before storing it.
  try {
    await validateHetznerToken(apiToken);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Hetzner rejected the API token" },
      400,
    );
  }

  const now = new Date();
  const encryptedToken = await encryptSecret(apiToken);

  await db
    .insert(hetznerConnection)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      apiToken: encryptedToken,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: hetznerConnection.organizationId,
      set: { apiToken: encryptedToken, updatedAt: now },
    });

  return c.json({
    connected: true,
    tokenHint: apiToken.slice(-4),
    updatedAt: now.toISOString(),
  } satisfies HetznerConnection);
});

hetzner.delete("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  await db.delete(hetznerConnection).where(eq(hetznerConnection.organizationId, organizationId));

  return c.body(null, 204);
});

hetzner.get("/locations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const token = await getHetznerToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a Hetzner Cloud API token in Secrets first" }, 400);
  }

  try {
    return c.json((await listHetznerLocations(token)) satisfies HetznerLocation[]);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't list Hetzner locations" },
      502,
    );
  }
});

hetzner.get("/server-types", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const token = await getHetznerToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a Hetzner Cloud API token in Secrets first" }, 400);
  }

  try {
    return c.json((await listHetznerServerTypes(token)) satisfies HetznerServerType[]);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't list Hetzner server types" },
      502,
    );
  }
});

const SERVER_NAME_PATTERN = /^[a-zA-Z0-9]([a-zA-Z0-9.-]{0,62}[a-zA-Z0-9])?$/;

// Creates a Hetzner Cloud server on the workspace's account and a linked server
// row. A background job waits for its public IP, then provisions the agent over
// SSH — the same pipeline as a manually added server.
hetzner.post("/servers", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateHetznerServerInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const location = typeof body?.location === "string" ? body.location.trim() : "";
  const serverType = typeof body?.serverType === "string" ? body.serverType.trim() : "";

  if (!name || !SERVER_NAME_PATTERN.test(name)) {
    return c.json({ error: "name must be a valid hostname (letters, digits, dots, dashes)" }, 400);
  }
  if (!location) return c.json({ error: "location is required" }, 400);
  if (!serverType) return c.json({ error: "serverType is required" }, 400);

  const token = await getHetznerToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a Hetzner Cloud API token in Secrets first" }, 400);
  }

  const id = crypto.randomUUID();
  const keyPair = await generateServerKeyPair(id);

  let hetznerServerId: string;
  try {
    hetznerServerId = await createHetznerCloudServer({
      token,
      name,
      location,
      serverType,
      sshPublicKey: keyPair.publicKey,
      serverId: id,
    });
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't create the Hetzner server" },
      502,
    );
  }

  const now = new Date();
  const [created] = await db
    .insert(server)
    .values({
      id,
      organizationId,
      name,
      // Placeholder until the server reports its public IP.
      sshHost: "0.0.0.0",
      sshPort: 22,
      sshUser: "root",
      sshPublicKey: keyPair.publicKey,
      sshPrivateKey: await encryptSecret(keyPair.privateKey),
      connectionMode: "ssh",
      provider: "hetzner",
      providerResourceId: hetznerServerId,
      status: "provisioning",
      statusMessage: "Waiting for the server to boot…",
      createdAt: now,
      updatedAt: now,
    })
    .returning({ id: server.id });

  if (!created) {
    return c.json({ error: "Failed to create server" }, 500);
  }

  try {
    await enqueueAction("hetzner-wait-server", id);
  } catch {
    await db
      .update(server)
      .set({
        status: "error",
        statusMessage:
          "Server created on Hetzner, but the wait job could not be queued (queue unavailable). Retry provisioning once it has an IP.",
        updatedAt: new Date(),
      })
      .where(eq(server.id, id));
  }

  return c.json({ id }, 201);
});
