import { alert, app as appTable, db, server } from "@basse/db";
import type { Alert, AlertsOverview } from "@basse/shared";
import { and, desc, eq, inArray, isNull, sql } from "drizzle-orm";
import { Hono } from "hono";
import { resolveActiveWorkspace } from "./workspace";

type AlertRow = typeof alert.$inferSelect;

export const alerts = new Hono();

function toAlert(row: {
  alert: AlertRow;
  serverName: string | null;
  appName: string | null;
}): Alert {
  return {
    id: row.alert.id,
    organizationId: row.alert.organizationId,
    severity: row.alert.severity,
    status: row.alert.status,
    code: row.alert.code,
    title: row.alert.title,
    message: row.alert.message,
    fingerprint: row.alert.fingerprint,
    serverId: row.alert.serverId,
    serverName: row.serverName,
    appId: row.alert.appId,
    appName: row.appName,
    deploymentId: row.alert.deploymentId,
    firstSeenAt: row.alert.firstSeenAt.toISOString(),
    lastSeenAt: row.alert.lastSeenAt.toISOString(),
    acknowledgedAt: row.alert.acknowledgedAt?.toISOString() ?? null,
    resolvedAt: row.alert.resolvedAt?.toISOString() ?? null,
    createdAt: row.alert.createdAt.toISOString(),
    updatedAt: row.alert.updatedAt.toISOString(),
  };
}

async function ownedAlert(alertId: string, organizationId: string): Promise<AlertRow | null> {
  const [row] = await db
    .select()
    .from(alert)
    .where(and(eq(alert.id, alertId), eq(alert.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
}

alerts.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const status = c.req.query("status") ?? "active";
  const where =
    status === "all"
      ? eq(alert.organizationId, organizationId)
      : status === "resolved"
        ? and(eq(alert.organizationId, organizationId), eq(alert.status, "resolved"))
        : and(eq(alert.organizationId, organizationId), inArray(alert.status, ["open", "acknowledged"]));

  const rows = await db
    .select({
      alert,
      serverName: server.name,
      appName: appTable.name,
    })
    .from(alert)
    .leftJoin(server, eq(alert.serverId, server.id))
    .leftJoin(appTable, eq(alert.appId, appTable.id))
    .where(where)
    .orderBy(desc(alert.lastSeenAt))
    .limit(200);

  return c.json(rows.map(toAlert));
});

alerts.get("/overview", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const [openCount, acknowledgedCount, criticalOpenCount] = await Promise.all([
    db
      .select({ count: sql<number>`count(*)::int` })
      .from(alert)
      .where(and(eq(alert.organizationId, organizationId), eq(alert.status, "open"))),
    db
      .select({ count: sql<number>`count(*)::int` })
      .from(alert)
      .where(and(eq(alert.organizationId, organizationId), eq(alert.status, "acknowledged"))),
    db
      .select({ count: sql<number>`count(*)::int` })
      .from(alert)
      .where(
        and(
          eq(alert.organizationId, organizationId),
          eq(alert.status, "open"),
          eq(alert.severity, "critical"),
        ),
      ),
  ]);

  return c.json({
    openCount: openCount[0]?.count ?? 0,
    acknowledgedCount: acknowledgedCount[0]?.count ?? 0,
    criticalOpenCount: criticalOpenCount[0]?.count ?? 0,
  } satisfies AlertsOverview);
});

alerts.post("/:id/acknowledge", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedAlert(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Alert not found" }, 404);
  if (row.status === "resolved") return c.json({ error: "Alert is already resolved" }, 400);

  await db
    .update(alert)
    .set({ status: "acknowledged", acknowledgedAt: new Date(), updatedAt: new Date() })
    .where(and(eq(alert.id, row.id), isNull(alert.resolvedAt)));

  return c.body(null, 204);
});

alerts.post("/:id/resolve", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedAlert(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Alert not found" }, 404);

  await db
    .update(alert)
    .set({ status: "resolved", resolvedAt: new Date(), updatedAt: new Date() })
    .where(eq(alert.id, row.id));

  return c.body(null, 204);
});
