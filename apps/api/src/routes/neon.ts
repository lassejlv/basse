import { db, neonConnection } from "@basse/db";
import type {
  CreateNeonBranchInput,
  NeonBranch,
  NeonBranchConnection,
  NeonConnection,
  NeonRegion,
  SaveNeonConnectionInput,
} from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import {
  createNeonBranch,
  deleteNeonBranch,
  getNeonApiKey,
  getNeonBranchConnection,
  listNeonBranches,
  listNeonRegions,
  validateNeonApiKey,
} from "../integrations/neon";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { resolveActiveWorkspace } from "../lib/workspace";
import { ownedApp } from "./apps";

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
    await validateNeonApiKey(apiKey);
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

// ── App-scoped branch management ─────────────────────────────────────────────

/** Loads the workspace-owned Neon app plus the decrypted API key, or a
 * Response to short-circuit with. */
async function resolveNeonApp(
  request: Request,
  appId: string,
): Promise<{ projectId: string; apiKey: string } | Response> {
  const organizationId = await resolveActiveWorkspace(request);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedApp(appId, organizationId);
  if (!row || row.appKind !== "neon" || !row.neonProjectId) {
    return Response.json({ error: "Neon database not found" }, { status: 404 });
  }

  const apiKey = await getNeonApiKey(organizationId);
  if (!apiKey) {
    return Response.json({ error: "Connect a Neon API key in Secrets first" }, { status: 400 });
  }

  return { projectId: row.neonProjectId, apiKey };
}

function neonFailure(error: unknown, fallback: string): Response {
  return Response.json(
    { error: error instanceof Error ? error.message : fallback },
    { status: 502 },
  );
}

neon.get("/apps/:appId/branches", async (c) => {
  const resolved = await resolveNeonApp(c.req.raw, c.req.param("appId"));
  if (resolved instanceof Response) return resolved;

  try {
    const branches = await listNeonBranches(resolved.apiKey, resolved.projectId);
    return c.json(branches satisfies NeonBranch[]);
  } catch (error) {
    return neonFailure(error, "Could not list Neon branches");
  }
});

neon.post("/apps/:appId/branches", async (c) => {
  const resolved = await resolveNeonApp(c.req.raw, c.req.param("appId"));
  if (resolved instanceof Response) return resolved;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateNeonBranchInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  if (!name) return c.json({ error: "name is required" }, 400);
  if (name.length > 64) return c.json({ error: "Branch name is too long" }, 400);

  try {
    const branch = await createNeonBranch(resolved.apiKey, resolved.projectId, name);
    return c.json(branch satisfies NeonBranch, 201);
  } catch (error) {
    return neonFailure(error, "Could not create the Neon branch");
  }
});

neon.delete("/apps/:appId/branches/:branchId", async (c) => {
  const resolved = await resolveNeonApp(c.req.raw, c.req.param("appId"));
  if (resolved instanceof Response) return resolved;

  try {
    await deleteNeonBranch(resolved.apiKey, resolved.projectId, c.req.param("branchId"));
    return c.body(null, 204);
  } catch (error) {
    return neonFailure(error, "Could not delete the Neon branch");
  }
});

neon.get("/apps/:appId/branches/:branchId/connection", async (c) => {
  const resolved = await resolveNeonApp(c.req.raw, c.req.param("appId"));
  if (resolved instanceof Response) return resolved;

  try {
    const connection = await getNeonBranchConnection(
      resolved.apiKey,
      resolved.projectId,
      c.req.param("branchId"),
    );
    return c.json(connection satisfies NeonBranchConnection);
  } catch (error) {
    return neonFailure(error, "Could not fetch the branch connection");
  }
});
