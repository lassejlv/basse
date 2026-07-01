import {
  app,
  appServer,
  db,
  domain,
  envVar,
  environment,
  project,
  server,
  stagedChange,
  stagedChangeHistory,
} from "@basse/db";
import type {
  AppStagedChanges,
  AppVolume,
  ApplyStagedChangesResult,
  EnvVarPlain,
  ProjectApplyStagedChangesResult,
  ProjectStagedChange,
  ProjectStagedChangeHistoryEntry,
  ProjectStagedChanges,
  PreviewDomainConfig,
  SetEnvVarsInput,
  StageDomainChangeInput,
  StagedChange,
  StagedChangeHistoryEntry,
  StagedChangeHistoryItem,
  StagedChangeHistoryOutcome,
  UpdateAppInput,
} from "@basse/shared";
import { and, asc, desc, eq, inArray } from "drizzle-orm";
import type { Context } from "hono";
import { Hono } from "hono";
import { buildAppUpdates, loadAppServerIds, ownedApp, slugify, toApp } from "./apps";
import {
  cloudPreviewEnabled,
  cloudPreviewReservedHostMessage,
  cloudPreviewRootDomain,
  deleteCloudPreviewDns,
  generatedCloudPreviewHost,
  isCloudPreviewHost,
  upsertCloudPreviewDns,
} from "./cloud-preview";
import { decryptSecret, encryptSecret } from "./crypto";
import { enqueueDeploy, toDeployment } from "./deployments";
import { validateHost, validateUpstream } from "./domains";
import { enqueueOrRunDomainSync } from "./proxy-sync";
import { publishRealtime } from "./realtime";
import { resolveActiveWorkspace } from "./workspace";

type AppRow = typeof app.$inferSelect;
type StagedChangeRow = typeof stagedChange.$inferSelect;
type StagedChangeHistoryRow = typeof stagedChangeHistory.$inferSelect;
type DbTransaction = Parameters<Parameters<typeof db.transaction>[0]>[0];
type DomainRow = typeof domain.$inferSelect;

type DomainChangePayload = {
  id: string | null;
  serverId: string;
  appId: string | null;
  host: string;
  upstream: string;
};

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

function uniqueConstraint(error: unknown): string | null {
  if (typeof error !== "object" || error === null) return null;
  const constraint = (error as { constraint?: unknown }).constraint;
  return typeof constraint === "string" ? constraint : null;
}

/** Standard response after any change to the staging set: the list + the draft. */
async function respondWithChanges(
  c: Context,
  existing: AppRow,
  options: { notify?: boolean; organizationId?: string; scope?: "stage" | "apply" } = {},
): Promise<Response> {
  const rows = await loadStagedRows(existing.id);
  const changes = await Promise.all(rows.map(toStagedChange));
  const draft = await buildDraft(existing, rows);
  if (options.notify && options.organizationId) {
    // The mutating client already received this exact state in the response
    // body, so its sockets are excluded from the event.
    publishRealtime(
      options.organizationId,
      { type: "staged-changes", appId: existing.id, scope: options.scope ?? "stage" },
      { excludeClient: c.req.header("x-basse-client") },
    );
  }
  return c.json({ changes, draft } satisfies AppStagedChanges);
}

function toHistoryItem(row: StagedChangeHistoryRow): StagedChangeHistoryItem {
  return {
    id: row.id,
    batchId: row.batchId,
    appId: row.appId,
    deploymentId: row.deploymentId,
    outcome: row.outcome,
    resource: row.resource,
    action: row.action,
    field: row.field,
    value: row.value,
    previousValue: row.previousValue,
    stagedAt: row.stagedAt.toISOString(),
    createdAt: row.createdAt.toISOString(),
  };
}

function toHistoryEntries(rows: StagedChangeHistoryRow[]): StagedChangeHistoryEntry[] {
  const entries = new Map<string, StagedChangeHistoryEntry>();

  for (const row of rows) {
    const item = toHistoryItem(row);
    const existing = entries.get(row.batchId);
    if (existing) {
      existing.changes.push(item);
      continue;
    }

    entries.set(row.batchId, {
      id: row.batchId,
      appId: row.appId,
      deploymentId: row.deploymentId,
      outcome: row.outcome,
      createdAt: row.createdAt.toISOString(),
      changes: [item],
    });
  }

  return [...entries.values()];
}

async function insertChangeHistory(
  tx: DbTransaction,
  rows: StagedChangeRow[],
  outcome: StagedChangeHistoryOutcome,
  now: Date,
): Promise<string | null> {
  if (rows.length === 0) return null;

  const batchId = crypto.randomUUID();
  const displayRows = await Promise.all(rows.map(toStagedChange));
  await tx.insert(stagedChangeHistory).values(
    displayRows.map((change, index) => {
      const row = rows[index]!;
      return {
        id: crypto.randomUUID(),
        batchId,
        appId: row.appId,
        deploymentId: null,
        outcome,
        resource: change.resource,
        action: change.action,
        field: change.field,
        value: change.value,
        previousValue: change.previousValue,
        stagedAt: row.createdAt,
        createdAt: now,
        updatedAt: now,
      };
    }),
  );

  return batchId;
}

async function applyStagedChangesForApp(
  existing: AppRow,
  organizationId: string,
): Promise<
  | { ok: true; result: ApplyStagedChangesResult }
  | { ok: false; error: string; status: 400 | 404 | 409 | 500 }
> {
  const rows = await loadStagedRows(existing.id);
  if (rows.length === 0) return { ok: false, error: "No changes to deploy", status: 400 };

  // Reconstruct the patch body from the staged app rows and re-validate it
  // through the SAME builder PATCH uses.
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
  if (!result.ok) return { ok: false, error: result.error, status: result.status };
  const updates = result.updates;
  const serverIds = result.serverIds;
  const currentServerIds = (await loadAppServerIds([existing.id])).get(existing.id) ?? [];
  const nextPort = typeof updates.port === "number" ? updates.port : existing.port;
  const nextUpstream = `basse-app-${existing.id}:${nextPort}`;

  const envRows = rows.filter((row) => row.resource === "env_var");
  const domainRows = rows.filter((row) => row.resource === "domain");
  const shouldDeploy = rows.some((row) => row.resource !== "domain");
  const stagedIds = rows.map((row) => row.id);
  const now = new Date();
  const domainSyncServerIds = new Set<string>();
  let historyBatchId: string | null = null;

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
        const domainServerRows = await tx
          .select({ serverId: domain.serverId })
          .from(domain)
          .where(eq(domain.appId, existing.id));
        const domainServerIds = [...new Set(domainServerRows.map((row) => row.serverId))];
        const currentSet = new Set(currentServerIds);
        const nextSet = new Set(serverIds);
        const removedDomainServerIds = domainServerIds.filter((serverId) => !nextSet.has(serverId));
        const singleServerMove =
          currentServerIds.length === 1 &&
          serverIds.length === 1 &&
          currentServerIds[0] !== serverIds[0];

        if (singleServerMove) {
          const oldServerId = currentServerIds[0]!;
          const newServerId = serverIds[0]!;
          const movedDomains = await tx
            .select()
            .from(domain)
            .where(and(eq(domain.appId, existing.id), eq(domain.serverId, oldServerId)));
          for (const movedDomain of movedDomains) {
            await upsertCloudPreviewDns(movedDomain.host, newServerId);
          }
          await tx
            .update(domain)
            .set({
              serverId: newServerId,
              upstream: nextUpstream,
              status: "pending",
              statusMessage: null,
              updatedAt: now,
            })
            .where(and(eq(domain.appId, existing.id), eq(domain.serverId, oldServerId)));
          domainSyncServerIds.add(oldServerId);
          domainSyncServerIds.add(newServerId);
          const staleServerIds = removedDomainServerIds.filter(
            (serverId) => serverId !== oldServerId,
          );
          if (staleServerIds.length > 0) {
            const staleDomains = await tx
              .select()
              .from(domain)
              .where(and(eq(domain.appId, existing.id), inArray(domain.serverId, staleServerIds)));
            for (const staleDomain of staleDomains) {
              await deleteCloudPreviewDns(staleDomain.host);
            }
            await tx
              .delete(domain)
              .where(and(eq(domain.appId, existing.id), inArray(domain.serverId, staleServerIds)));
            for (const serverId of staleServerIds) domainSyncServerIds.add(serverId);
          }
        } else if (removedDomainServerIds.length > 0) {
          const removedDomains = await tx
            .select()
            .from(domain)
            .where(
              and(eq(domain.appId, existing.id), inArray(domain.serverId, removedDomainServerIds)),
            );
          for (const removedDomain of removedDomains) {
            await deleteCloudPreviewDns(removedDomain.host);
          }
          await tx
            .delete(domain)
            .where(
              and(eq(domain.appId, existing.id), inArray(domain.serverId, removedDomainServerIds)),
            );
          for (const serverId of removedDomainServerIds) domainSyncServerIds.add(serverId);
        }

        for (const serverId of serverIds) {
          if (!currentSet.has(serverId)) domainSyncServerIds.add(serverId);
        }
      }
      if (typeof updates.port === "number") {
        const domainServerRows = await tx
          .select({ serverId: domain.serverId })
          .from(domain)
          .where(eq(domain.appId, existing.id));
        await tx
          .update(domain)
          .set({ upstream: nextUpstream, status: "pending", statusMessage: null, updatedAt: now })
          .where(eq(domain.appId, existing.id));
        for (const row of domainServerRows) domainSyncServerIds.add(row.serverId);
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
      for (const row of domainRows) {
        if (row.action === "delete") {
          const payload = parseDomainChangePayload(row.previousValue);
          if (!payload) continue;
          await deleteCloudPreviewDns(payload.host);
          await tx
            .delete(domain)
            .where(
              and(
                eq(domain.serverId, payload.serverId),
                eq(domain.host, payload.host),
                eq(domain.appId, existing.id),
              ),
            );
          domainSyncServerIds.add(payload.serverId);
          continue;
        }

        if (!row.value) continue;
        const payload = parseDomainChangePayload(row.value);
        if (!payload) continue;
        const serverAttached = await appServerBelongsToWorkspaceApp(
          payload.serverId,
          existing.id,
          organizationId,
        );
        if (!serverAttached) throw new Error("Domain server is not attached to this app");
        await upsertCloudPreviewDns(payload.host, payload.serverId);
        if (row.action === "update") {
          await tx
            .update(domain)
            .set({
              upstream: payload.upstream,
              status: "pending",
              statusMessage: null,
              updatedAt: now,
            })
            .where(
              and(
                eq(domain.serverId, payload.serverId),
                eq(domain.host, payload.host),
                eq(domain.appId, existing.id),
              ),
            );
          domainSyncServerIds.add(payload.serverId);
          continue;
        }
        await tx.insert(domain).values({
          id: payload.id ?? crypto.randomUUID(),
          serverId: payload.serverId,
          appId: existing.id,
          host: payload.host,
          upstream: payload.upstream,
          status: "pending",
          statusMessage: null,
          createdAt: now,
          updatedAt: now,
        });
        domainSyncServerIds.add(payload.serverId);
      }
      historyBatchId = await insertChangeHistory(tx, rows, "applied", now);
      // Clear only the rows we actually applied; anything staged concurrently
      // (between the read above and this commit) survives to be applied later.
      await tx.delete(stagedChange).where(inArray(stagedChange.id, stagedIds));
    });
  } catch (error) {
    if (isUniqueViolation(error)) {
      if (uniqueConstraint(error)?.includes("domain_serverId_host")) {
        return { ok: false, error: "That host is already in use", status: 409 };
      }
      return {
        ok: false,
        error: "An app with that name already exists in this environment",
        status: 409,
      };
    }
    if (error instanceof Error && error.message === "Domain server is not attached to this app") {
      return { ok: false, error: error.message, status: 400 };
    }
    if (error instanceof Error && error.message.startsWith("Cloud preview")) {
      const status = error.message.includes("Cloudflare:") ? 500 : 400;
      return { ok: false, error: error.message, status };
    }
    if (error instanceof Error && error.message.startsWith("Cloudflare:")) {
      return { ok: false, error: `Cloud preview DNS sync failed: ${error.message}`, status: 500 };
    }
    throw error;
  }

  await Promise.all([...domainSyncServerIds].map((serverId) => enqueueOrRunDomainSync(serverId)));

  let deployment: ApplyStagedChangesResult["deployment"] = null;
  if (shouldDeploy) {
    const deployResult = await enqueueDeploy(existing.id);
    deployment = "deployment" in deployResult ? toDeployment(deployResult.deployment) : null;
  }
  if (historyBatchId && deployment) {
    await db
      .update(stagedChangeHistory)
      .set({ deploymentId: deployment.id, updatedAt: new Date() })
      .where(eq(stagedChangeHistory.batchId, historyBatchId));
  }

  return { ok: true, result: { deployment, domainSyncs: domainSyncServerIds.size } };
}

async function loadProjectApps(projectId: string, organizationId: string) {
  return db
    .select({
      app,
      environmentId: environment.id,
      environmentName: environment.name,
    })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(project.id, projectId), eq(project.organizationId, organizationId)))
    .orderBy(asc(environment.createdAt), asc(app.createdAt));
}

async function ownedProject(projectId: string, organizationId: string): Promise<boolean> {
  const [row] = await db
    .select({ id: project.id })
    .from(project)
    .where(and(eq(project.id, projectId), eq(project.organizationId, organizationId)))
    .limit(1);
  return Boolean(row);
}

async function loadProjectStagedChanges(
  projectId: string,
  organizationId: string,
): Promise<ProjectStagedChange[]> {
  const appRows = await loadProjectApps(projectId, organizationId);
  if (appRows.length === 0) return [];

  const appContext = new Map(appRows.map((row) => [row.app.id, row]));
  const rows = await db
    .select()
    .from(stagedChange)
    .where(
      inArray(
        stagedChange.appId,
        appRows.map((row) => row.app.id),
      ),
    )
    .orderBy(asc(stagedChange.createdAt));

  const changes = await Promise.all(rows.map(toStagedChange));
  return changes.flatMap((change): ProjectStagedChange[] => {
    const context = appContext.get(change.appId);
    if (!context) return [];
    return [
      {
        ...change,
        appName: context.app.name,
        environmentId: context.environmentId,
        environmentName: context.environmentName,
      },
    ];
  });
}

async function loadProjectHistory(
  projectId: string,
  organizationId: string,
): Promise<ProjectStagedChangeHistoryEntry[]> {
  const appRows = await loadProjectApps(projectId, organizationId);
  if (appRows.length === 0) return [];

  const appContext = new Map(appRows.map((row) => [row.app.id, row]));
  const rows = await db
    .select()
    .from(stagedChangeHistory)
    .where(
      inArray(
        stagedChangeHistory.appId,
        appRows.map((row) => row.app.id),
      ),
    )
    .orderBy(desc(stagedChangeHistory.createdAt), asc(stagedChangeHistory.stagedAt))
    .limit(300);

  return toHistoryEntries(rows).flatMap((entry): ProjectStagedChangeHistoryEntry[] => {
    const context = appContext.get(entry.appId);
    if (!context) return [];
    return [
      {
        ...entry,
        appName: context.app.name,
        environmentId: context.environmentId,
        environmentName: context.environmentName,
      },
    ];
  });
}

function jsonEqual(a: unknown, b: unknown): boolean {
  return JSON.stringify(a ?? null) === JSON.stringify(b ?? null);
}

export const changes = new Hono();
export const projectChanges = new Hono();

projectChanges.get("/:id/changes", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProject(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const changes = await loadProjectStagedChanges(projectId, organizationId);
  publishRealtime(
    organizationId,
    { type: "staged-changes", projectId, scope: "apply" },
    { excludeClient: c.req.header("x-basse-client") },
  );
  return c.json({ changes } satisfies ProjectStagedChanges);
});

projectChanges.get("/:id/changes/history", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProject(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  return c.json(await loadProjectHistory(projectId, organizationId));
});

projectChanges.post("/:id/changes/apply", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProject(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const appRows = await loadProjectApps(projectId, organizationId);
  const pending = await loadProjectStagedChanges(projectId, organizationId);
  if (pending.length === 0) return c.json({ error: "No changes to deploy" }, 400);

  const appIds = [...new Set(pending.map((change) => change.appId))];
  const deployments: ProjectApplyStagedChangesResult["deployments"] = [];
  for (const appId of appIds) {
    const context = appRows.find((row) => row.app.id === appId);
    if (!context) continue;
    const result = await applyStagedChangesForApp(context.app, organizationId);
    if (!result.ok) {
      return c.json({ error: `${context.app.name}: ${result.error}` }, result.status);
    }
    deployments.push({
      appId: context.app.id,
      appName: context.app.name,
      deployment: result.result.deployment,
      domainSyncs: result.result.domainSyncs,
    });
  }

  publishRealtime(
    organizationId,
    { type: "staged-changes", projectId, scope: "apply" },
    { excludeClient: c.req.header("x-basse-client") },
  );
  return c.json({ deployments } satisfies ProjectApplyStagedChangesResult);
});

projectChanges.post("/:id/changes/discard", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProject(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const appRows = await loadProjectApps(projectId, organizationId);
  const appIds = appRows.map((row) => row.app.id);
  if (appIds.length === 0) {
    return c.json({ changes: [] } satisfies ProjectStagedChanges);
  }

  const rows = await db
    .select()
    .from(stagedChange)
    .where(inArray(stagedChange.appId, appIds))
    .orderBy(asc(stagedChange.createdAt));
  if (rows.length > 0) {
    const now = new Date();
    await db.transaction(async (tx) => {
      for (const appId of appIds) {
        const appChangeRows = rows.filter((row) => row.appId === appId);
        await insertChangeHistory(tx, appChangeRows, "discarded", now);
      }
      await tx.delete(stagedChange).where(inArray(stagedChange.appId, appIds));
    });
  }

  const changes = await loadProjectStagedChanges(projectId, organizationId);
  publishRealtime(
    organizationId,
    { type: "staged-changes", projectId, scope: "apply" },
    { excludeClient: c.req.header("x-basse-client") },
  );
  return c.json({ changes } satisfies ProjectStagedChanges);
});

projectChanges.delete("/:id/changes/:changeId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProject(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const [row] = await db
    .select({ stagedChange })
    .from(stagedChange)
    .innerJoin(app, eq(stagedChange.appId, app.id))
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(
      and(
        eq(stagedChange.id, c.req.param("changeId")),
        eq(project.id, projectId),
        eq(project.organizationId, organizationId),
      ),
    )
    .limit(1);

  if (row) {
    const now = new Date();
    await db.transaction(async (tx) => {
      await insertChangeHistory(tx, [row.stagedChange], "discarded", now);
      await tx.delete(stagedChange).where(eq(stagedChange.id, row.stagedChange.id));
    });
  }

  const changes = await loadProjectStagedChanges(projectId, organizationId);
  publishRealtime(
    organizationId,
    { type: "staged-changes", projectId, scope: "apply" },
    { excludeClient: c.req.header("x-basse-client") },
  );
  return c.json({ changes } satisfies ProjectStagedChanges);
});

// GET /api/apps/:id/changes — the pending changes plus the draft app.
changes.get("/:id/changes", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  return respondWithChanges(c, existing);
});

// GET /api/apps/:id/changes/history — recent applied/discarded staged batches.
changes.get("/:id/changes/history", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const rows = await db
    .select()
    .from(stagedChangeHistory)
    .where(eq(stagedChangeHistory.appId, existing.id))
    .orderBy(desc(stagedChangeHistory.createdAt), asc(stagedChangeHistory.stagedAt))
    .limit(200);

  return c.json(toHistoryEntries(rows));
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

  return respondWithChanges(c, existing, { notify: true, organizationId });
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

  return respondWithChanges(c, existing, { notify: true, organizationId });
});

// GET /api/apps/:id/changes/preview-domain — cloud preview URL settings for
// single-server apps. Disabled in self-hosted installs unless env is configured.
changes.get("/:id/changes/preview-domain", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  return c.json(await buildPreviewDomainConfig(existing));
});

// POST /api/apps/:id/changes/preview-domain — stage the one allowed managed
// preview domain. Applying the staged change writes the Cloudflare A record.
changes.post("/:id/changes/preview-domain", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);
  if (!cloudPreviewEnabled()) {
    return c.json({ error: "Cloud preview domains are not enabled" }, 404);
  }

  const currentServerIds = (await loadAppServerIds([existing.id])).get(existing.id) ?? [];
  if (currentServerIds.length !== 1) {
    return c.json({ error: "Preview domains require exactly one attached server" }, 400);
  }

  const livePreview = await loadLivePreviewDomain(existing.id);
  if (livePreview) {
    return c.json({ error: "This app already has a preview domain" }, 409);
  }

  const rows = await loadStagedRows(existing.id);
  const draft = await buildDraft(existing, rows);
  const stagedPreview = findStagedPreviewPayload(rows);
  const host = generatedCloudPreviewHost(draft.slug, existing.id);
  if (!host) return c.json({ error: "Cloud preview domains are not enabled" }, 404);

  const serverId = currentServerIds[0]!;
  const upstream = `basse-app-${existing.id}:${draft.port}`;
  if (stagedPreview) {
    if (stagedPreview.host === host && stagedPreview.serverId === serverId) {
      return respondWithChanges(c, existing, { notify: true, organizationId });
    }
    return c.json({ error: "This app already has a staged preview domain" }, 409);
  }

  const now = new Date();
  await upsertDomainChange(
    db,
    existing.id,
    "create",
    domainChangeField(serverId, host),
    serializeDomainChangePayload({
      id: null,
      serverId,
      appId: existing.id,
      host,
      upstream,
    }),
    null,
    now,
  );

  return respondWithChanges(c, existing, { notify: true, organizationId });
});

// POST /api/apps/:id/changes/domain — stage a domain create/delete. Applying
// the batch commits the domain table change and queues a proxy sync.
changes.post("/:id/changes/domain", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as Partial<StageDomainChangeInput> | null;
  const now = new Date();

  if (body?.action === "create") {
    const serverId = typeof body.serverId === "string" ? body.serverId : "";
    const host = typeof body.host === "string" ? body.host.trim().toLowerCase() : "";
    const upstream = typeof body.upstream === "string" ? body.upstream.trim() : "";
    const hostError = validateHost(host);
    if (hostError) return c.json({ error: hostError }, 400);
    if (isCloudPreviewHost(host)) {
      return c.json({ error: cloudPreviewReservedHostMessage() }, 400);
    }
    const upstreamError = validateUpstream(upstream);
    if (upstreamError) return c.json({ error: upstreamError }, 400);
    if (!(await appServerBelongsToWorkspaceApp(serverId, existing.id, organizationId))) {
      return c.json({ error: "Server is not attached to this app" }, 400);
    }

    const [liveDomain] = await db
      .select()
      .from(domain)
      .where(and(eq(domain.serverId, serverId), eq(domain.host, host)))
      .limit(1);
    const field = domainChangeField(serverId, host);

    if (liveDomain) {
      if (liveDomain.appId !== existing.id) {
        return c.json({ error: "That host is already in use" }, 409);
      }
      if (liveDomain.upstream === upstream) {
        await db
          .delete(stagedChange)
          .where(
            and(
              eq(stagedChange.appId, existing.id),
              eq(stagedChange.resource, "domain"),
              eq(stagedChange.field, field),
            ),
          );
        return respondWithChanges(c, existing, { notify: true, organizationId });
      }
      await upsertDomainChange(
        db,
        existing.id,
        "update",
        field,
        serializeDomainChangePayload({ ...domainToPayload(liveDomain), upstream }),
        serializeDomainChangePayload(domainToPayload(liveDomain)),
        now,
      );
      return respondWithChanges(c, existing, { notify: true, organizationId });
    }

    await upsertDomainChange(
      db,
      existing.id,
      "create",
      field,
      serializeDomainChangePayload({
        id: null,
        serverId,
        appId: existing.id,
        host,
        upstream,
      }),
      null,
      now,
    );
    return respondWithChanges(c, existing, { notify: true, organizationId });
  }

  if (body?.action === "delete") {
    const domainId = typeof body.domainId === "string" ? body.domainId : "";
    const [liveDomain] = await db
      .select({ domain })
      .from(domain)
      .innerJoin(server, eq(domain.serverId, server.id))
      .where(
        and(
          eq(domain.id, domainId),
          eq(domain.appId, existing.id),
          eq(server.organizationId, organizationId),
        ),
      )
      .limit(1);

    if (!liveDomain) return c.json({ error: "Domain not found" }, 404);

    await upsertDomainChange(
      db,
      existing.id,
      "delete",
      domainChangeField(liveDomain.domain.serverId, liveDomain.domain.host),
      null,
      serializeDomainChangePayload(domainToPayload(liveDomain.domain)),
      now,
    );
    return respondWithChanges(c, existing, { notify: true, organizationId });
  }

  return c.json({ error: "Invalid domain change" }, 400);
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

  const result = await applyStagedChangesForApp(existing, organizationId);
  if (!result.ok) return c.json({ error: result.error }, result.status);
  publishRealtime(
    organizationId,
    { type: "staged-changes", appId: existing.id, scope: "apply" },
    { excludeClient: c.req.header("x-basse-client") },
  );
  return c.json(result.result);
});

// POST /api/apps/:id/changes/discard — drop every staged change for the app.
changes.post("/:id/changes/discard", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const rows = await loadStagedRows(existing.id);
  if (rows.length > 0) {
    const now = new Date();
    await db.transaction(async (tx) => {
      await insertChangeHistory(tx, rows, "discarded", now);
      await tx.delete(stagedChange).where(eq(stagedChange.appId, existing.id));
    });
  }
  return respondWithChanges(c, existing, { notify: true, organizationId, scope: "apply" });
});

// DELETE /api/apps/:id/changes/:changeId — discard a single staged change.
changes.delete("/:id/changes/:changeId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedApp(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "App not found" }, 404);

  const [row] = await db
    .select()
    .from(stagedChange)
    .where(and(eq(stagedChange.id, c.req.param("changeId")), eq(stagedChange.appId, existing.id)))
    .limit(1);

  if (row) {
    const now = new Date();
    await db.transaction(async (tx) => {
      await insertChangeHistory(tx, [row], "discarded", now);
      await tx.delete(stagedChange).where(eq(stagedChange.id, row.id));
    });
  }
  return respondWithChanges(c, existing, { notify: true, organizationId, scope: "apply" });
});

async function buildPreviewDomainConfig(existing: AppRow): Promise<PreviewDomainConfig> {
  const rootDomain = cloudPreviewRootDomain();
  if (!rootDomain || !cloudPreviewEnabled()) {
    return { enabled: false, rootDomain, host: null };
  }

  const rows = await loadStagedRows(existing.id);
  const draft = await buildDraft(existing, rows);
  const livePreview = await loadLivePreviewDomain(existing.id);
  const stagedPreview = findStagedPreviewPayload(rows);
  return {
    enabled: true,
    rootDomain,
    host:
      livePreview?.host ??
      stagedPreview?.host ??
      generatedCloudPreviewHost(draft.slug, existing.id),
  };
}

async function loadLivePreviewDomain(appId: string): Promise<DomainRow | null> {
  const rows = await db
    .select()
    .from(domain)
    .where(eq(domain.appId, appId))
    .orderBy(domain.createdAt);
  return rows.find((row) => isCloudPreviewHost(row.host)) ?? null;
}

function findStagedPreviewPayload(rows: StagedChangeRow[]): DomainChangePayload | null {
  for (const row of rows) {
    if (row.resource !== "domain" || row.action === "delete") continue;
    const payload = parseDomainChangePayload(row.value);
    if (payload && isCloudPreviewHost(payload.host)) return payload;
  }
  return null;
}

function upsertAppChange(
  tx: DbTransaction,
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

function domainChangeField(serverId: string, host: string): string {
  return `${serverId}:${host}`;
}

function domainToPayload(row: DomainRow): DomainChangePayload {
  return {
    id: row.id,
    serverId: row.serverId,
    appId: row.appId,
    host: row.host,
    upstream: row.upstream,
  };
}

function serializeDomainChangePayload(payload: DomainChangePayload): string {
  return JSON.stringify(payload);
}

function parseDomainChangePayload(value: string | null): DomainChangePayload | null {
  if (!value) return null;
  try {
    const parsed = JSON.parse(value) as Partial<DomainChangePayload>;
    if (
      typeof parsed.serverId !== "string" ||
      typeof parsed.host !== "string" ||
      typeof parsed.upstream !== "string"
    ) {
      return null;
    }
    return {
      id: typeof parsed.id === "string" ? parsed.id : null,
      serverId: parsed.serverId,
      appId: typeof parsed.appId === "string" ? parsed.appId : null,
      host: parsed.host,
      upstream: parsed.upstream,
    };
  } catch {
    return null;
  }
}

async function appServerBelongsToWorkspaceApp(
  serverId: string,
  appId: string,
  organizationId: string,
): Promise<boolean> {
  const [row] = await db
    .select({ id: appServer.serverId })
    .from(appServer)
    .innerJoin(server, eq(appServer.serverId, server.id))
    .where(
      and(
        eq(appServer.appId, appId),
        eq(appServer.serverId, serverId),
        eq(server.organizationId, organizationId),
      ),
    )
    .limit(1);
  return Boolean(row);
}

function upsertDomainChange(
  tx: typeof db,
  appId: string,
  action: "create" | "update" | "delete",
  field: string,
  value: string | null,
  previousValue: string | null,
  now: Date,
): Promise<unknown> {
  return tx
    .insert(stagedChange)
    .values({
      id: crypto.randomUUID(),
      appId,
      resource: "domain",
      action,
      field,
      value,
      previousValue,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: [stagedChange.appId, stagedChange.resource, stagedChange.field],
      set: { action, value, previousValue, updatedAt: now },
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
