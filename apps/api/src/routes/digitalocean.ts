import { db, digitaloceanConnection, server } from "@basse/db";
import type {
  CreateDigitalOceanServerInput,
  DigitalOceanConnection,
  DigitalOceanRegion,
  DigitalOceanSize,
  SaveDigitalOceanConnectionInput,
} from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import {
  createDigitalOceanDroplet,
  getDigitalOceanToken,
  listDigitalOceanRegions,
  listDigitalOceanSizes,
  validateDigitalOceanToken,
} from "../integrations/digitalocean";
import { generateServerKeyPair } from "../infra/server-keys";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { resolveActiveWorkspace } from "../lib/workspace";
import { enqueueAction } from "../queue/queue";

export const digitalocean = new Hono();

digitalocean.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const [connection] = await db
    .select()
    .from(digitaloceanConnection)
    .where(eq(digitaloceanConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) {
    return c.json({ connected: false } satisfies DigitalOceanConnection);
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
  } satisfies DigitalOceanConnection);
});

digitalocean.put("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req
    .json()
    .catch(() => null)) as Partial<SaveDigitalOceanConnectionInput> | null;
  const apiToken = typeof body?.apiToken === "string" ? body.apiToken.trim() : "";
  if (!apiToken) {
    return c.json({ error: "apiToken is required" }, 400);
  }

  // Round-trip the token against the DigitalOcean API before storing it.
  try {
    await validateDigitalOceanToken(apiToken);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "DigitalOcean rejected the API token" },
      400,
    );
  }

  const now = new Date();
  const encryptedToken = await encryptSecret(apiToken);

  await db
    .insert(digitaloceanConnection)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      apiToken: encryptedToken,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: digitaloceanConnection.organizationId,
      set: { apiToken: encryptedToken, updatedAt: now },
    });

  return c.json({
    connected: true,
    tokenHint: apiToken.slice(-4),
    updatedAt: now.toISOString(),
  } satisfies DigitalOceanConnection);
});

digitalocean.delete("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  await db
    .delete(digitaloceanConnection)
    .where(eq(digitaloceanConnection.organizationId, organizationId));

  return c.body(null, 204);
});

digitalocean.get("/regions", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const token = await getDigitalOceanToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a DigitalOcean API token in Secrets first" }, 400);
  }

  try {
    return c.json((await listDigitalOceanRegions(token)) satisfies DigitalOceanRegion[]);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't list DigitalOcean regions" },
      502,
    );
  }
});

digitalocean.get("/sizes", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const token = await getDigitalOceanToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a DigitalOcean API token in Secrets first" }, 400);
  }

  try {
    return c.json((await listDigitalOceanSizes(token)) satisfies DigitalOceanSize[]);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't list DigitalOcean sizes" },
      502,
    );
  }
});

const SERVER_NAME_PATTERN = /^[a-zA-Z0-9]([a-zA-Z0-9.-]{0,62}[a-zA-Z0-9])?$/;

// Creates a droplet on the workspace's DigitalOcean account and a linked server
// row. A background job waits for the droplet's public IP, then provisions the
// agent over SSH — the same pipeline as a manually added server.
digitalocean.post("/servers", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req
    .json()
    .catch(() => null)) as Partial<CreateDigitalOceanServerInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const region = typeof body?.region === "string" ? body.region.trim() : "";
  const size = typeof body?.size === "string" ? body.size.trim() : "";

  if (!name || !SERVER_NAME_PATTERN.test(name)) {
    return c.json({ error: "name must be a valid hostname (letters, digits, dots, dashes)" }, 400);
  }
  if (!region) return c.json({ error: "region is required" }, 400);
  if (!size) return c.json({ error: "size is required" }, 400);

  const token = await getDigitalOceanToken(organizationId);
  if (!token) {
    return c.json({ error: "Connect a DigitalOcean API token in Secrets first" }, 400);
  }

  const id = crypto.randomUUID();
  const keyPair = await generateServerKeyPair(id);

  let dropletId: string;
  try {
    dropletId = await createDigitalOceanDroplet({
      token,
      name,
      region,
      size,
      sshPublicKey: keyPair.publicKey,
      serverId: id,
    });
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Couldn't create the droplet" },
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
      // Placeholder until the droplet reports its public IP.
      sshHost: "0.0.0.0",
      sshPort: 22,
      sshUser: "root",
      sshPublicKey: keyPair.publicKey,
      sshPrivateKey: await encryptSecret(keyPair.privateKey),
      connectionMode: "ssh",
      provider: "digitalocean",
      providerResourceId: dropletId,
      status: "provisioning",
      statusMessage: "Waiting for the droplet to boot…",
      createdAt: now,
      updatedAt: now,
    })
    .returning({ id: server.id });

  if (!created) {
    return c.json({ error: "Failed to create server" }, 500);
  }

  try {
    await enqueueAction("digitalocean-wait-server", id);
  } catch {
    await db
      .update(server)
      .set({
        status: "error",
        statusMessage:
          "Droplet created, but the wait job could not be queued (queue unavailable). Retry provisioning once it has an IP.",
        updatedAt: new Date(),
      })
      .where(eq(server.id, id));
  }

  return c.json({ id }, 201);
});
