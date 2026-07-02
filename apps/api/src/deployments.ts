import { app, appServer, db, deployment } from "@basse/db";
import type { RollbackDeploymentInput, TriggerDeploymentInput } from "@basse/shared";
import type { Deployment } from "@basse/shared";
import { and, desc, eq, inArray, isNotNull } from "drizzle-orm";
import { Hono } from "hono";
import { ownedApp } from "./apps";
import { enqueueAction } from "./queue/queue";
import { resolveActiveWorkspace } from "./workspace";

type DeploymentRow = typeof deployment.$inferSelect;

export function toDeployment(row: DeploymentRow): Deployment {
  return {
    id: row.id,
    appId: row.appId,
    status: row.status,
    phase: row.phase,
    commitSha: row.commitSha,
    imageRef: row.imageRef,
    buildId: row.buildId,
    buildNoCache: row.buildNoCache,
    logs: row.logs,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

/**
 * Creates a queued deployment for an app and enqueues the build/deploy job.
 * Returns the created row, or a structured error (no server attached, or the
 * queue is unavailable). Shared by POST / and the staged-changes apply route.
 */
export async function enqueueDeploy(
  appId: string,
  options: Omit<TriggerDeploymentInput, "appId"> = {},
): Promise<{ deployment: DeploymentRow } | { error: string; status: 400 | 503 }> {
  if (options.useLatestImage && options.noCache) {
    return { error: "Choose either skip build or no-cache build, not both", status: 400 };
  }

  const [appRow] = await db.select().from(app).where(eq(app.id, appId)).limit(1);
  if (!appRow) {
    return { error: "App not found", status: 400 };
  }
  if (options.noCache && appRow.sourceType !== "repository") {
    return { error: "No-cache deploys are only available for repository apps", status: 400 };
  }

  const targetServers = await db
    .select({ serverId: appServer.serverId })
    .from(appServer)
    .where(eq(appServer.appId, appId));
  if (targetServers.length === 0) {
    return { error: "Attach at least one server to the app before deploying", status: 400 };
  }

  let seed: Pick<DeploymentRow, "commitSha" | "imageRef" | "buildId" | "logs"> = {
    commitSha: null,
    imageRef: null,
    buildId: null,
    logs: null,
  };
  if (options.useLatestImage) {
    const [latestImage] = await db
      .select({
        id: deployment.id,
        commitSha: deployment.commitSha,
        imageRef: deployment.imageRef,
        buildId: deployment.buildId,
      })
      .from(deployment)
      .where(
        and(
          eq(deployment.appId, appId),
          inArray(deployment.status, ["healthy", "superseded"]),
          isNotNull(deployment.imageRef),
        ),
      )
      .orderBy(desc(deployment.createdAt))
      .limit(1);
    if (!latestImage?.imageRef) {
      return { error: "No previous deployment image is available to redeploy", status: 400 };
    }
    seed = {
      commitSha: latestImage.commitSha,
      imageRef: latestImage.imageRef,
      buildId: latestImage.buildId,
      logs: `Redeploying saved image from deployment ${latestImage.id} (${latestImage.imageRef}).\n`,
    };
  }

  const now = new Date();
  const [created] = await db
    .insert(deployment)
    .values({
      id: crypto.randomUUID(),
      appId,
      status: "queued",
      commitSha: seed.commitSha,
      imageRef: seed.imageRef,
      buildId: seed.buildId,
      buildNoCache: options.noCache === true,
      logs: seed.logs,
      createdAt: now,
      updatedAt: now,
    })
    .returning();
  if (!created) return { error: "Failed to create deployment", status: 503 };

  try {
    await enqueueAction("deploy-app", created.id);
  } catch {
    await db
      .update(deployment)
      .set({ status: "failed", logs: "Could not queue the deployment.", updatedAt: new Date() })
      .where(eq(deployment.id, created.id));
    return { error: "Could not queue the deployment", status: 503 };
  }

  return { deployment: created };
}

/** Loads a deployment only if its app belongs to the active workspace. */
async function ownedDeployment(
  deploymentId: string,
  organizationId: string,
): Promise<DeploymentRow | null> {
  const [row] = await db.select().from(deployment).where(eq(deployment.id, deploymentId)).limit(1);
  if (!row) return null;
  const owned = await ownedApp(row.appId, organizationId);
  return owned ? row : null;
}

export const deployments = new Hono();

deployments.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.query("appId");
  if (!appId || !(await ownedApp(appId, organizationId))) {
    return c.json({ error: "App not found" }, 404);
  }

  // The list is refetched on every realtime deployment event, so it must stay
  // light: build logs (which grow to hundreds of KB) are only included for the
  // newest row and any in-flight rows — the ones whose logs the UI streams.
  // Older rows return logs: null; the detail endpoint serves them on demand.
  const rows = await db
    .select({
      id: deployment.id,
      appId: deployment.appId,
      status: deployment.status,
      phase: deployment.phase,
      commitSha: deployment.commitSha,
      imageRef: deployment.imageRef,
      buildId: deployment.buildId,
      buildNoCache: deployment.buildNoCache,
      createdAt: deployment.createdAt,
      updatedAt: deployment.updatedAt,
    })
    .from(deployment)
    .where(eq(deployment.appId, appId))
    .orderBy(desc(deployment.createdAt))
    .limit(50);

  const logIds = rows
    .filter((row, index) => index === 0 || ["queued", "building", "deploying"].includes(row.status))
    .map((row) => row.id);
  const logRows = logIds.length
    ? await db
        .select({ id: deployment.id, logs: deployment.logs })
        .from(deployment)
        .where(inArray(deployment.id, logIds))
    : [];
  const logsById = new Map(logRows.map((row) => [row.id, row.logs]));

  return c.json(rows.map((row) => toDeployment({ ...row, logs: logsById.get(row.id) ?? null })));
});

deployments.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as Partial<TriggerDeploymentInput> | null;
  const appId = typeof body?.appId === "string" ? body.appId : "";

  const appRow = await ownedApp(appId, organizationId);
  if (!appRow) return c.json({ error: "App not found" }, 404);

  const result = await enqueueDeploy(appRow.id, {
    useLatestImage: body?.useLatestImage === true,
    noCache: body?.noCache === true,
  });
  if ("error" in result) return c.json({ error: result.error }, result.status);

  return c.json(toDeployment(result.deployment), 201);
});

deployments.post("/rollback", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as RollbackDeploymentInput | null;
  const deploymentId = typeof body?.deploymentId === "string" ? body.deploymentId : "";

  const target = await ownedDeployment(deploymentId, organizationId);
  if (!target) return c.json({ error: "Deployment not found" }, 404);
  if (!target.imageRef) {
    return c.json({ error: "Deployment does not have a saved image to roll back to" }, 400);
  }
  if (!["healthy", "superseded"].includes(target.status)) {
    return c.json({ error: "Only healthy or superseded deployments can be rolled back" }, 400);
  }

  const targetServers = await db
    .select({ serverId: appServer.serverId })
    .from(appServer)
    .where(eq(appServer.appId, target.appId));
  if (targetServers.length === 0) {
    return c.json({ error: "Attach at least one server to the app before rolling back" }, 400);
  }

  const now = new Date();
  const [created] = await db
    .insert(deployment)
    .values({
      id: crypto.randomUUID(),
      appId: target.appId,
      status: "queued",
      commitSha: target.commitSha,
      imageRef: target.imageRef,
      logs: `Rolling back to deployment ${target.id} (${target.imageRef}).\n`,
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) return c.json({ error: "Failed to create rollback deployment" }, 500);

  try {
    await enqueueAction("deploy-app", created.id);
  } catch {
    await db
      .update(deployment)
      .set({
        status: "failed",
        logs: `${created.logs ?? ""}Could not queue the rollback.\n`,
        updatedAt: new Date(),
      })
      .where(and(eq(deployment.id, created.id), eq(deployment.status, "queued")));
    return c.json({ error: "Could not queue the rollback" }, 503);
  }

  return c.json(toDeployment(created), 201);
});

deployments.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedDeployment(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Deployment not found" }, 404);

  return c.json(toDeployment(row));
});
