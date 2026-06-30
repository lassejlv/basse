import { db, envVar } from "@basse/db";
import type { EnvVarMasked, SetEnvVarsInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { ownedApp } from "./apps";
import { decryptSecret, encryptSecret } from "./crypto";
import { resolveActiveWorkspace } from "./workspace";

async function maskedValue(encrypted: string): Promise<string> {
  try {
    const value = await decryptSecret(encrypted);
    return value.length <= 4 ? "••••" : `••••${value.slice(-4)}`;
  } catch {
    return "••••";
  }
}

export const envVars = new Hono();

// GET /api/apps/:appId/env-vars — keys + masked values only (never plaintext).
envVars.get("/:appId/env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedApp(c.req.param("appId"), organizationId);
  if (!owned) return c.json({ error: "App not found" }, 404);

  const rows = await db
    .select()
    .from(envVar)
    .where(eq(envVar.appId, owned.id))
    .orderBy(envVar.key);

  const masked: EnvVarMasked[] = await Promise.all(
    rows.map(async (row) => ({
      key: row.key,
      valueHint: await maskedValue(row.value),
      updatedAt: row.updatedAt.toISOString(),
    })),
  );

  return c.json(masked);
});

// PUT /api/apps/:appId/env-vars — bulk replace the whole set (encrypted at rest).
envVars.put("/:appId/env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedApp(c.req.param("appId"), organizationId);
  if (!owned) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as Partial<SetEnvVarsInput> | null;
  const vars = Array.isArray(body?.vars) ? body.vars : null;
  if (!vars) return c.json({ error: "vars must be an array" }, 400);

  const cleaned: { key: string; value: string }[] = [];
  for (const v of vars) {
    const key = typeof v?.key === "string" ? v.key.trim() : "";
    const value = typeof v?.value === "string" ? v.value : "";
    if (!key) continue;
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      return c.json({ error: `invalid variable name: ${key}` }, 400);
    }
    cleaned.push({ key, value });
  }

  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.delete(envVar).where(eq(envVar.appId, owned.id));
    if (cleaned.length > 0) {
      await tx.insert(envVar).values(
        await Promise.all(
          cleaned.map(async (v) => ({
            id: crypto.randomUUID(),
            appId: owned.id,
            key: v.key,
            value: await encryptSecret(v.value),
            createdAt: now,
            updatedAt: now,
          })),
        ),
      );
    }
  });

  return c.body(null, 204);
});

// DELETE /api/apps/:appId/env-vars/:key — remove one variable.
envVars.delete("/:appId/env-vars/:key", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedApp(c.req.param("appId"), organizationId);
  if (!owned) return c.json({ error: "App not found" }, 404);

  await db
    .delete(envVar)
    .where(and(eq(envVar.appId, owned.id), eq(envVar.key, c.req.param("key"))));

  return c.body(null, 204);
});
