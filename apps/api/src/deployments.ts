import { db, deployment } from "@basse/db";
import type { Deployment } from "@basse/shared";
import { desc, eq } from "drizzle-orm";
import { Hono } from "hono";
import { ownedApp } from "./apps";
import { resolveActiveWorkspace } from "./workspace";

type DeploymentRow = typeof deployment.$inferSelect;

export function toDeployment(row: DeploymentRow): Deployment {
  return {
    id: row.id,
    appId: row.appId,
    status: row.status,
    commitSha: row.commitSha,
    imageRef: row.imageRef,
    buildId: row.buildId,
    logs: row.logs,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

/** Loads a deployment only if its app belongs to the active workspace. */
async function ownedDeployment(
  deploymentId: string,
  organizationId: string,
): Promise<DeploymentRow | null> {
  const [row] = await db
    .select()
    .from(deployment)
    .where(eq(deployment.id, deploymentId))
    .limit(1);
  if (!row) return null;
  const owned = await ownedApp(row.appId, organizationId);
  return owned ? row : null;
}

export const deployments = new Hono();

deployments.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.query("appId");
  if (!appId || !(await ownedApp(appId, organizationId))) {
    return c.json({ error: "App not found" }, 404);
  }

  const rows = await db
    .select()
    .from(deployment)
    .where(eq(deployment.appId, appId))
    .orderBy(desc(deployment.createdAt));

  return c.json(rows.map(toDeployment));
});

deployments.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedDeployment(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Deployment not found" }, 404);

  return c.json(toDeployment(row));
});
