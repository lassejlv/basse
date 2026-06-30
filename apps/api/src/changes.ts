import { app, appServer, db, envVar, stagedChange } from "@basse/db";
import type {
  AppStagedChanges,
  AppVolume,
  EnvVarPlain,
  SetEnvVarsInput,
  StagedChange,
  UpdateAppInput,
} from "@basse/shared";
import { and, asc, eq, inArray } from "drizzle-orm";
import type { Context } from "hono";
import { Hono } from "hono";
import { buildAppUpdates, loadAppServerIds, ownedApp, slugify, toApp } from "./apps";
import { decryptSecret, encryptSecret } from "./crypto";
import { enqueueDeploy, toDeployment } from "./deployments";
import { resolveActiveWorkspace } from "./workspace";

type AppRow = typeof app.$inferSelect;
type StagedChangeRow = typeof stagedChange.$inferSelect;

/** Last-4 masked hint for an encrypted env value; never returns plaintext. */
async function maskedValue(encrypted: string): Promise<string> {
  try {
    const value = await decryptSecret(encrypted);
    return value.length <= 4 ? "••••" : `••••${value.slice(-4)}`;
  } catch {
    return "••••";
  }
}

/** Serializes a staged-change row into the wire DTO (env values are masked). */
async function toStagedChange(row: StagedChangeRow): Promise<StagedChange> {
  const masked = row.resource === "env_var";
  return {
    id: row.id,
    appId: row.appId,
    resource: row.resource,
    action: row.action,
    field: row.field,
    value: masked ? (row.value ? await maskedValue(row.value) : null) : row.value,
    previousValue: masked
      ? row.previousValue
        ? await maskedValue(row.previousValue)
        : null
      : row.previousValue,
    createdAt: row.createdAt.toISOString(),
  };
}

function loadStagedRows(appId: string): Promise<StagedChangeRow[]> {
  return db
    .select()
    .from(stagedChange)
    .where(eq(stagedChange.appId, appId))
    .orderBy(asc(stagedChange.createdAt));
}

/** Builds the draft App = the live row with staged app-config changes overlaid. */
async function buildDraft(
  existing: AppRow,
  rows: StagedChangeRow[],
): Promise<ReturnType<typeof toApp>> {
  const draftRow: AppRow = { ...existing };
  let draftServerIds: string[] | null = null;
  for (const row of rows) {
    if (row.resource !== "app" || row.value === null) continue;
    if (row.field === "serverIds") {
      draftServerIds = JSON.parse(row.value) as string[];
      continue;
    }
    (draftRow as Record<string, unknown>)[row.field] = JSON.parse(row.value);
  }
  // slug is derived from name (never staged), so keep the draft consistent with
  // what apply will write.
  draftRow.slug = slugify(draftRow.name);
  if (draftServerIds) draftRow.serverId = draftServerIds[0] ?? null;
  const serverIds =
    draftServerIds ?? (await loadAppServerIds([existing.id])).get(existing.id) ?? [];
  return toApp(draftRow, serverIds);
}

/** True for a Postgres unique-constraint violation (duplicate app slug, etc.). */
function isUniqueViolation(error: unknown): boolean {
  if (typeof error !== "object" || error === null) return false;
  if ((error as { code?: unknown }).code === "23505") return true;
  const message = (error as { message?: unknown }).message;
  return typeof message === "string" && message.includes("duplicate key value");
}

/** Standard response after any change to the staging set: the list + the draft. */
async function respondWithChanges(c: Context, existing: AppRow): Promise<Response> {
  const rows = await loadStagedRows(existing.id);
  const changes = await Promise.all(rows.map(toStagedChange));
  const draft = await buildDraft(existing, rows);
  return c.json({ changes, draft } satisfies AppStagedChanges);
}

function jsonEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

export const changes = new Hono();

// GET /api/apps/:id/changes — the pending changes plus the draft app.
changes.get("/:id/changes", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  return respondWithChanges(c, existing);
});

// GET /api/apps/:id/changes/env-draft — draft env (current ⊕ staged) plaintext,
// so the env editor edits on top of what is already staged. Same auth gate as
// the reveal endpoint; the user owns these secrets.
changes.get("/:id/changes/env-draft", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const map = new Map<string, string>();
  const currentRows = await db.select().from(envVar).where(eq(envVar.appId, existing.id));
  for (const row of currentRows) map.set(row.key, await decryptSecret(row.value));

  const stagedRows = await db
    .select()
    .from(stagedChange)
    .where(and(eq(stagedChange.appId, existing.id), eq(stagedChange.resource, "env_var")));
  for (const row of stagedRows) {
    if (row.action === "delete") {
      map.delete(row.field);
    } else if (row.value) {
      map.set(row.field, await decryptSecret(row.value));
    }
  }

  const draft: EnvVarPlain[] = [...map]
    .map(([key, value]) => ({ key, value }))
    .sort((a, b) => a.key.localeCompare(b.key));
  return c.json(draft);
});

// POST /api/apps/:id/changes/app — stage a partial app-config patch. Validation
// matches PATCH; only fields that actually differ from the live app are staged,
// and a field re-set to its original value clears its staged row.
changes.post("/:id/changes/app", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as UpdateAppInput | null;
  const result = await buildAppUpdates(existing, body, organizationId);
  if (!result.ok) return c.json({ error: result.error }, result.status);

  const currentServerIds = (await loadAppServerIds([existing.id])).get(existing.id) ?? [];
  const now = new Date();

  await db.transaction(async (tx) => {
    for (const [field, newValue] of Object.entries(result.updates)) {
      // serverId is derived from the serverIds set (staged under "serverIds")
      // and slug is derived from name on apply — neither is staged directly.
      if (field === "serverId" || field === "slug" || field === "updatedAt") continue;
      const oldValue = (existing as Record<string, unknown>)[field];
      if (jsonEqual(newValue, oldValue)) {
        await tx
          .delete(stagedChange)
          .where(
            and(
              eq(stagedChange.appId, existing.id),
              eq(stagedChange.resource, "app"),
              eq(stagedChange.field, field),
            ),
          );
        continue;
      }
      await upsertAppChange(
        tx,
        existing.id,
        field,
        JSON.stringify(newValue),
        JSON.stringify(oldValue ?? null),
        now,
      );
    }

    if (result.serverIds) {
      const sortedNew = [...result.serverIds].sort();
      const sortedCurrent = [...currentServerIds].sort();
      if (jsonEqual(sortedNew, sortedCurrent)) {
        await tx
          .delete(stagedChange)
          .where(
            and(
              eq(stagedChange.appId, existing.id),
              eq(stagedChange.resource, "app"),
              eq(stagedChange.field, "serverIds"),
            ),
          );
      } else {
        await upsertAppChange(
          tx,
          existing.id,
          "serverIds",
          JSON.stringify(result.serverIds),
          JSON.stringify(currentServerIds),
          now,
        );
      }
    }
  });

  return respondWithChanges(c, existing);
});

// POST /api/apps/:id/changes/env — stage the full desired env-var set. The set
// is diffed against the live vars into create/update/delete rows; values are
// encrypted at rest exactly like the live env_var table.
changes.post("/:id/changes/env", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as Partial<SetEnvVarsInput> | null;
  const vars = Array.isArray(body?.vars) ? body.vars : null;
  if (!vars) return c.json({ error: "vars must be an array" }, 400);

  const desired = new Map<string, string>();
  for (const v of vars) {
    const key = typeof v?.key === "string" ? v.key.trim() : "";
    const value = typeof v?.value === "string" ? v.value : "";
    if (!key) continue;
    if (!/^[A-Za-z_][A-Za-z0-9_]*$/.test(key)) {
      return c.json({ error: `invalid variable name: ${key}` }, 400);
    }
    desired.set(key, value);
  }

  const currentRows = await db.select().from(envVar).where(eq(envVar.appId, existing.id));
  const current = new Map<string, { encrypted: string; plain: string }>();
  for (const row of currentRows) {
    current.set(row.key, { encrypted: row.value, plain: await decryptSecret(row.value) });
  }

  const now = new Date();
  const inserts: (typeof stagedChange.$inferInsert)[] = [];
  for (const [key, value] of desired) {
    const existingVar = current.get(key);
    if (!existingVar) {
      inserts.push(envChange(existing.id, "create", key, await encryptSecret(value), null, now));
    } else if (existingVar.plain !== value) {
      inserts.push(
        envChange(
          existing.id,
          "update",
          key,
          await encryptSecret(value),
          existingVar.encrypted,
          now,
        ),
      );
    }
  }
  for (const [key, existingVar] of current) {
    if (desired.has(key)) continue;
    inserts.push(envChange(existing.id, "delete", key, null, existingVar.encrypted, now));
  }

  // The editor always submits the full desired set, so recompute env staging
  // wholesale: drop the old env rows and insert the freshly diffed ones.
  await db.transaction(async (tx) => {
    await tx
      .delete(stagedChange)
      .where(and(eq(stagedChange.appId, existing.id), eq(stagedChange.resource, "env_var")));
    if (inserts.length > 0) await tx.insert(stagedChange).values(inserts);
  });

  return respondWithChanges(c, existing);
});

// POST /api/apps/:id/changes/apply — commit every staged change to the live
// app/env tables in one transaction, clear the staging set, then trigger a
// deploy (which reads the now-updated config). Returns the deployment, or null
// when no server is attached to deploy to.
changes.post("/:id/changes/apply", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const rows = await loadStagedRows(existing.id);
  if (rows.length === 0) return c.json({ error: "No changes to deploy" }, 400);

  // Reconstruct the patch body from the staged app rows and re-validate it
  // through the SAME builder PATCH uses. A partially-discarded set (e.g.
  // sourceType=image with its imageRef row removed) is rejected here instead of
  // persisting an invalid/undeployable config, and slug/imageRef/server checks
  // stay identical to stage time.
  const body: UpdateAppInput = {};
  for (const row of rows) {
    if (row.resource !== "app" || row.value === null) continue;
    const parsed = JSON.parse(row.value) as unknown;
    if (row.field === "serverIds") {
      body.serverIds = parsed as string[];
    } else if (row.field === "volumes") {
      // The volumes column is itself a JSON string, so the staged value is
      // double-encoded: parse once to the column string, then to the array.
      body.volumes = JSON.parse(parsed as string) as AppVolume[];
    } else {
      (body as Record<string, unknown>)[row.field] = parsed;
    }
  }

  const result = await buildAppUpdates(existing, body, organizationId);
  if (!result.ok) return c.json({ error: result.error }, result.status);
  const updates = result.updates;
  const serverIds = result.serverIds;

  const envRows = rows.filter((row) => row.resource === "env_var");
  const stagedIds = rows.map((row) => row.id);
  const now = new Date();

  try {
    await db.transaction(async (tx) => {
      if (Object.keys(updates).length > 0) {
        await tx
          .update(app)
          .set({ ...updates, updatedAt: now })
          .where(eq(app.id, existing.id));
      }
      if (serverIds) {
        await tx.delete(appServer).where(eq(appServer.appId, existing.id));
        if (serverIds.length > 0) {
          await tx
            .insert(appServer)
            .values(
              serverIds.map((serverId) => ({ appId: existing.id, serverId, createdAt: now })),
            );
        }
      }
      for (const row of envRows) {
        if (row.action === "delete") {
          await tx
            .delete(envVar)
            .where(and(eq(envVar.appId, existing.id), eq(envVar.key, row.field)));
          continue;
        }
        if (!row.value) continue;
        await tx
          .insert(envVar)
          .values({
            id: crypto.randomUUID(),
            appId: existing.id,
            key: row.field,
            value: row.value,
            createdAt: now,
            updatedAt: now,
          })
          .onConflictDoUpdate({
            target: [envVar.appId, envVar.key],
            set: { value: row.value, updatedAt: now },
          });
      }
      // Clear only the rows we actually applied; anything staged concurrently
      // (between the read above and this commit) survives to be applied later.
      await tx.delete(stagedChange).where(inArray(stagedChange.id, stagedIds));
    });
  } catch (error) {
    if (isUniqueViolation(error)) {
      return c.json({ error: "An app with that name already exists in this environment" }, 409);
    }
    throw error;
  }

  const deployResult = await enqueueDeploy(existing.id);
  const deployment = "deployment" in deployResult ? toDeployment(deployResult.deployment) : null;
  return c.json({ deployment });
});

// POST /api/apps/:id/changes/discard — drop every staged change for the app.
changes.post("/:id/changes/discard", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  await db.delete(stagedChange).where(eq(stagedChange.appId, existing.id));
  return respondWithChanges(c, existing);
});

// DELETE /api/apps/:id/changes/:changeId — discard a single staged change.
changes.delete("/:id/changes/:changeId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  await db
    .delete(stagedChange)
    .where(and(eq(stagedChange.id, c.req.param("changeId")), eq(stagedChange.appId, existing.id)));
  return respondWithChanges(c, existing);
});

function upsertAppChange(
  tx: Parameters<Parameters<typeof db.transaction>[0]>[0],
  appId: string,
  field: string,
  value: string,
  previousValue: string,
  now: Date,
): Promise<unknown> {
  return tx
    .insert(stagedChange)
    .values({
      id: crypto.randomUUID(),
      appId,
      resource: "app",
      action: "update",
      field,
      value,
      previousValue,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: [stagedChange.appId, stagedChange.resource, stagedChange.field],
      set: { value, previousValue, action: "update", updatedAt: now },
    });
}

function envChange(
  appId: string,
  action: "create" | "update" | "delete",
  field: string,
  value: string | null,
  previousValue: string | null,
  now: Date,
): typeof stagedChange.$inferInsert {
  return {
    id: crypto.randomUUID(),
    appId,
    resource: "env_var",
    action,
    field,
    value,
    previousValue,
    createdAt: now,
    updatedAt: now,
  };
}
