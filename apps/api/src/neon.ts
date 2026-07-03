import { db, neonConnection } from "@basse/db";
import type { NeonConnection, NeonRegion, SaveNeonConnectionInput } from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import { resolveActiveWorkspace } from "./workspace";

const NEON_API = "https://console.neon.tech/api/v2";

type NeonRegionResponse = {
  region_id: string;
  name: string;
};

type NeonCreateProjectResponse = {
  project: { id: string; region_id: string };
  connection_uris: { connection_uri: string }[];
};

async function neonRequest(apiKey: string, path: string, init?: RequestInit): Promise<Response> {
  return fetch(`${NEON_API}${path}`, {
    ...init,
    headers: {
      accept: "application/json",
      authorization: `Bearer ${apiKey}`,
      ...(init?.body ? { "content-type": "application/json" } : {}),
      ...init?.headers,
    },
  });
}

async function neonError(response: Response, fallback: string): Promise<string> {
  const body = (await response.json().catch(() => null)) as { message?: string } | null;
  return body?.message || `${fallback} (${response.status})`;
}

export async function listNeonRegions(apiKey: string): Promise<NeonRegion[]> {
  const response = await neonRequest(apiKey, "/regions");
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not list Neon regions"));
  }
  const body = (await response.json()) as { regions?: NeonRegionResponse[] };
  return (body.regions ?? []).map((region) => ({ id: region.region_id, name: region.name }));
}

export async function createNeonProject(
  apiKey: string,
  input: { name: string; regionId: string },
): Promise<{ projectId: string; regionId: string; connectionUri: string }> {
  const response = await neonRequest(apiKey, "/projects", {
    method: "POST",
    body: JSON.stringify({ project: { name: input.name, region_id: input.regionId } }),
  });
  if (!response.ok) {
    throw new Error(await neonError(response, "Could not create the Neon project"));
  }
  const body = (await response.json()) as NeonCreateProjectResponse;
  const connectionUri = body.connection_uris[0]?.connection_uri;
  if (!connectionUri) {
    throw new Error("Neon did not return a connection string for the new project");
  }
  return {
    projectId: body.project.id,
    regionId: body.project.region_id,
    connectionUri,
  };
}

export async function deleteNeonProject(apiKey: string, projectId: string): Promise<void> {
  const response = await neonRequest(apiKey, `/projects/${encodeURIComponent(projectId)}`, {
    method: "DELETE",
  });
  // Already gone on Neon's side is fine — the goal is that it no longer exists.
  if (!response.ok && response.status !== 404) {
    throw new Error(await neonError(response, "Could not delete the Neon project"));
  }
}

/** The workspace's decrypted Neon API key, or null when not connected. */
export async function getNeonApiKey(organizationId: string): Promise<string | null> {
  const [connection] = await db
    .select()
    .from(neonConnection)
    .where(eq(neonConnection.organizationId, organizationId))
    .limit(1);
  if (!connection) return null;
  return decryptSecret(connection.apiKey);
}

export const neon = new Hono();

neon.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const [connection] = await db
    .select()
    .from(neonConnection)
    .where(eq(neonConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) {
    return c.json({ connected: false } satisfies NeonConnection);
  }

  let keyHint = "";
  try {
    const apiKey = await decryptSecret(connection.apiKey);
    keyHint = apiKey.slice(-4);
  } catch {
    keyHint = "";
  }

  return c.json({
    connected: true,
    keyHint,
    updatedAt: connection.updatedAt.toISOString(),
  } satisfies NeonConnection);
});

neon.put("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<SaveNeonConnectionInput> | null;
  const apiKey = typeof body?.apiKey === "string" ? body.apiKey.trim() : "";
  if (!apiKey) {
    return c.json({ error: "apiKey is required" }, 400);
  }

  // Round-trip the key against the Neon API before storing it.
  try {
    await listNeonRegions(apiKey);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Neon rejected the API key" },
      400,
    );
  }

  const now = new Date();
  const encryptedKey = await encryptSecret(apiKey);

  await db
    .insert(neonConnection)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      apiKey: encryptedKey,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: neonConnection.organizationId,
      set: { apiKey: encryptedKey, updatedAt: now },
    });

  return c.json({
    connected: true,
    keyHint: apiKey.slice(-4),
    updatedAt: now.toISOString(),
  } satisfies NeonConnection);
});

neon.delete("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  await db.delete(neonConnection).where(eq(neonConnection.organizationId, organizationId));

  return c.body(null, 204);
});

neon.get("/regions", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const apiKey = await getNeonApiKey(organizationId);
  if (!apiKey) {
    return c.json({ error: "Connect a Neon API key in Secrets first" }, 400);
  }

  try {
    return c.json((await listNeonRegions(apiKey)) satisfies NeonRegion[]);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Could not list Neon regions" },
      502,
    );
  }
});
