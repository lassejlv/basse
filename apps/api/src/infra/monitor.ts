import {
  alert,
  app,
  appServer,
  db,
  deployment,
  environment,
  monitorEvent,
  project,
  server,
} from "@basse/db";
import type { MonitorSeverity } from "@basse/shared";
import { and, desc, eq, inArray, lt } from "drizzle-orm";
import { checkAgentHealth, getAppMetrics, getAppStatus } from "./agent-client";
import { decryptSecret } from "../lib/crypto";
import { sendAlertEmail } from "../lib/email";
import { probeAppHttp } from "../routes/health";
import { publishForDeployment, publishRealtime } from "./realtime";
import { connectionFromServer } from "./server-connection";

type ServerRow = typeof server.$inferSelect;

export type MonitorIssue = {
  organizationId: string;
  severity: MonitorSeverity;
  code: string;
  title: string;
  message: string;
  fingerprint: string;
  serverId?: string | null;
  appId?: string | null;
  deploymentId?: string | null;
};

const IN_FLIGHT_DEPLOYMENTS = ["queued", "building", "deploying"] as const;
const pressureCounts = new Map<string, number>();
// HTTP health check pacing (per app+server) and consecutive-failure counters.
const healthProbeAt = new Map<string, number>();
const healthFailCounts = new Map<string, number>();
const HEALTH_FAILURE_THRESHOLD = 2;

function envNumber(name: string, fallback: number): number {
  const value = Number(Bun.env[name]);
  return Number.isFinite(value) && value > 0 ? value : fallback;
}

const MONITOR_INTERVAL_MS = envNumber("MONITOR_INTERVAL_SECONDS", 60) * 1000;
const DEPLOYMENT_STUCK_MINUTES = envNumber("MONITOR_DEPLOYMENT_STUCK_MINUTES", 30);
const CPU_PRESSURE_PERCENT = envNumber("MONITOR_CPU_PRESSURE_PERCENT", 90);
const MEMORY_PRESSURE_PERCENT = envNumber("MONITOR_MEMORY_PRESSURE_PERCENT", 90);
const PRESSURE_FAILURE_THRESHOLD = envNumber("MONITOR_RESOURCE_FAILURE_THRESHOLD", 3);
const OUTBOUND_HEARTBEAT_GRACE_MS =
  envNumber("MONITOR_OUTBOUND_HEARTBEAT_GRACE_SECONDS", 90) * 1000;

export async function recordEvent(issue: MonitorIssue): Promise<void> {
  await db.insert(monitorEvent).values({
    id: crypto.randomUUID(),
    organizationId: issue.organizationId,
    severity: issue.severity,
    code: issue.code,
    title: issue.title,
    message: issue.message,
    fingerprint: issue.fingerprint,
    serverId: issue.serverId ?? null,
    appId: issue.appId ?? null,
    deploymentId: issue.deploymentId ?? null,
    createdAt: new Date(),
  });
}

export async function raiseAlert(issue: MonitorIssue): Promise<void> {
  const now = new Date();
  const [existing] = await db
    .select()
    .from(alert)
    .where(
      and(
        eq(alert.organizationId, issue.organizationId),
        eq(alert.fingerprint, issue.fingerprint),
        inArray(alert.status, ["open", "acknowledged"]),
      ),
    )
    .limit(1);

  if (existing) {
    await db
      .update(alert)
      .set({
        severity: issue.severity,
        title: issue.title,
        message: issue.message,
        lastSeenAt: now,
        updatedAt: now,
      })
      .where(eq(alert.id, existing.id));
    // No realtime publish here: this branch re-bumps an already-open alert on
    // every monitor tick, which would make clients refetch once a minute.
    return;
  }

  const [created] = await db
    .insert(alert)
    .values({
      id: crypto.randomUUID(),
      organizationId: issue.organizationId,
      severity: issue.severity,
      status: "open",
      code: issue.code,
      title: issue.title,
      message: issue.message,
      fingerprint: issue.fingerprint,
      serverId: issue.serverId ?? null,
      appId: issue.appId ?? null,
      deploymentId: issue.deploymentId ?? null,
      firstSeenAt: now,
      lastSeenAt: now,
      createdAt: now,
      updatedAt: now,
    })
    .returning();
  await recordEvent(issue);
  publishRealtime(issue.organizationId, { type: "alert" });
  if (created) {
    await sendAlertEmail({
      id: created.id,
      organizationId: created.organizationId,
      severity: created.severity,
      title: created.title,
      message: created.message,
      code: created.code,
      fingerprint: created.fingerprint,
    }).catch((error) => {
      console.error("[monitor] alert email failed", error instanceof Error ? error.message : error);
    });
  }
}

export async function resolveAlert(
  organizationId: string,
  fingerprint: string,
  recovery: Omit<MonitorIssue, "organizationId" | "fingerprint">,
): Promise<void> {
  const rows = await db
    .select()
    .from(alert)
    .where(
      and(
        eq(alert.organizationId, organizationId),
        eq(alert.fingerprint, fingerprint),
        inArray(alert.status, ["open", "acknowledged"]),
      ),
    );

  if (rows.length === 0) return;

  const now = new Date();
  await db
    .update(alert)
    .set({ status: "resolved", resolvedAt: now, updatedAt: now })
    .where(
      and(
        eq(alert.organizationId, organizationId),
        eq(alert.fingerprint, fingerprint),
        inArray(alert.status, ["open", "acknowledged"]),
      ),
    );
  publishRealtime(organizationId, { type: "alert" });

  await recordEvent({
    organizationId,
    fingerprint,
    ...recovery,
  });
}

async function checkServer(row: ServerRow): Promise<boolean> {
  if (!row.agentToken) return false;

  const fingerprint = `server_unreachable:${row.id}`;
  const now = new Date();
  let health: { reachable: boolean; ready: boolean; error?: string };

  if (row.connectionMode === "outbound") {
    const lastSeenAt = row.lastSeenAt?.getTime() ?? 0;
    const ageMs = lastSeenAt ? now.getTime() - lastSeenAt : Number.POSITIVE_INFINITY;
    const fresh = ageMs <= OUTBOUND_HEARTBEAT_GRACE_MS;
    health = {
      reachable: fresh,
      ready: fresh,
      error: lastSeenAt
        ? `Outbound agent has not polled in ${Math.round(ageMs / 1000)}s`
        : "Outbound agent has not connected yet",
    };
  } else {
    const token = await decryptSecret(row.agentToken);
    health = await checkAgentHealth(await connectionFromServer(row), token, { attempts: 1 });
  }

  if (!health.reachable || !health.ready) {
    await db
      .update(server)
      .set({
        status: "unreachable",
        statusMessage: health.error ?? "Basse agent is not reachable",
        updatedAt: now,
      })
      .where(eq(server.id, row.id));
    await raiseAlert({
      organizationId: row.organizationId,
      severity: "critical",
      code: "server_unreachable",
      title: `${row.name} is unreachable`,
      message: health.error ?? "Basse could not reach the server agent.",
      fingerprint,
      serverId: row.id,
    });
    return false;
  }

  await db
    .update(server)
    .set({
      status: "active",
      statusMessage: null,
      lastSeenAt: now,
      updatedAt: now,
    })
    .where(eq(server.id, row.id));
  await resolveAlert(row.organizationId, fingerprint, {
    severity: "info",
    code: "server_recovered",
    title: `${row.name} recovered`,
    message: "Basse can reach the server agent again.",
    serverId: row.id,
  });
  await resolveAlert(row.organizationId, `server_monitor_failed:${row.id}`, {
    severity: "info",
    code: "server_monitor_recovered",
    title: `${row.name} monitoring recovered`,
    message: "The server monitor check is succeeding again.",
    serverId: row.id,
  });
  return true;
}

async function checkStuckDeployments(): Promise<void> {
  // Staleness is judged by updatedAt: a live build bumps it constantly via the
  // log writer and status transitions, so "no writes for the window" means the
  // job is dead (crashed worker, lost lock). Dead runs can't resume — their
  // build temp dir and pull token are gone — so mark them failed rather than
  // leaving a zombie "building" row the UI shows forever.
  const cutoff = new Date(Date.now() - DEPLOYMENT_STUCK_MINUTES * 60_000);
  const rows = await db
    .select({
      deployment,
      app,
      project,
    })
    .from(deployment)
    .innerJoin(app, eq(deployment.appId, app.id))
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(inArray(deployment.status, IN_FLIGHT_DEPLOYMENTS), lt(deployment.updatedAt, cutoff)))
    .limit(100);

  for (const row of rows) {
    const now = new Date();
    await db
      .update(deployment)
      .set({
        status: "failed",
        logs: `${row.deployment.logs ?? ""}\nMarked failed by the monitor: no progress for more than ${DEPLOYMENT_STUCK_MINUTES} minutes.\n`,
        updatedAt: now,
      })
      .where(
        and(
          eq(deployment.id, row.deployment.id),
          inArray(deployment.status, [...IN_FLIGHT_DEPLOYMENTS]),
        ),
      );
    void publishForDeployment(row.deployment.id);
    await raiseAlert({
      organizationId: row.project.organizationId,
      severity: "warning",
      code: "deployment_stuck",
      title: `${row.app.name} deployment stalled`,
      message: `Deployment ${row.deployment.id.slice(0, 8)} made no progress for more than ${DEPLOYMENT_STUCK_MINUTES} minutes and was marked failed.`,
      fingerprint: `deployment_stuck:${row.deployment.id}`,
      appId: row.app.id,
      deploymentId: row.deployment.id,
    });
  }
}

async function resolveFinishedDeploymentAlerts(): Promise<void> {
  const rows = await db
    .select({ alert, deployment, app, project })
    .from(alert)
    .innerJoin(deployment, eq(alert.deploymentId, deployment.id))
    .innerJoin(app, eq(deployment.appId, app.id))
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(
      and(eq(alert.code, "deployment_stuck"), inArray(alert.status, ["open", "acknowledged"])),
    );

  for (const row of rows) {
    if (
      IN_FLIGHT_DEPLOYMENTS.includes(
        row.deployment.status as (typeof IN_FLIGHT_DEPLOYMENTS)[number],
      )
    ) {
      continue;
    }
    await resolveAlert(row.project.organizationId, row.alert.fingerprint, {
      severity: "info",
      code: "deployment_recovered",
      title: `${row.app.name} deployment finished`,
      message: `Deployment ${row.deployment.id.slice(0, 8)} is now ${row.deployment.status}.`,
      appId: row.app.id,
      deploymentId: row.deployment.id,
    });
  }
}

async function checkAppsOnServer(row: ServerRow): Promise<void> {
  if (!row.agentToken) return;
  const token = await decryptSecret(row.agentToken);
  const conn = await connectionFromServer(row);

  const attachedApps = await db
    .select({
      app,
      project,
    })
    .from(appServer)
    .innerJoin(app, eq(appServer.appId, app.id))
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(appServer.serverId, row.id));

  for (const attached of attachedApps) {
    const [latest] = await db
      .select()
      .from(deployment)
      .where(eq(deployment.appId, attached.app.id))
      .orderBy(desc(deployment.createdAt))
      .limit(1);
    if (!latest || latest.status !== "healthy") continue;

    const downFingerprint = `app_container_down:${attached.app.id}:${row.id}`;
    try {
      const status = await getAppStatus(conn, token, attached.app.id);
      await resolveAlert(
        attached.project.organizationId,
        `app_monitor_failed:${attached.app.id}:${row.id}`,
        {
          severity: "info",
          code: "app_monitor_recovered",
          title: `${attached.app.name} monitoring recovered`,
          message: "The app monitor check is succeeding again.",
          serverId: row.id,
          appId: attached.app.id,
          deploymentId: latest.id,
        },
      );
      if (!status.exists || !status.running) {
        await raiseAlert({
          organizationId: attached.project.organizationId,
          severity: "critical",
          code: "app_container_down",
          title: `${attached.app.name} is not running on ${row.name}`,
          message: status.exists
            ? "The managed container exists but is not running."
            : "The managed container does not exist on the target server.",
          fingerprint: downFingerprint,
          serverId: row.id,
          appId: attached.app.id,
          deploymentId: latest.id,
        });
        continue;
      }
      await resolveAlert(attached.project.organizationId, downFingerprint, {
        severity: "info",
        code: "app_container_recovered",
        title: `${attached.app.name} recovered on ${row.name}`,
        message: "The managed container is running again.",
        serverId: row.id,
        appId: attached.app.id,
        deploymentId: latest.id,
      });

      // Continuous HTTP health check for service apps that configured one,
      // paced by the app's interval (never faster than the monitor tick).
      // Alerts after two consecutive failures to ride out one-off blips.
      if (attached.app.healthCheckEnabled && attached.app.appKind === "service") {
        const probeKey = `${attached.app.id}:${row.id}`;
        const intervalMs = attached.app.healthCheckIntervalSeconds * 1000;
        const lastProbe = healthProbeAt.get(probeKey) ?? 0;
        if (Date.now() - lastProbe >= intervalMs) {
          healthProbeAt.set(probeKey, Date.now());
          const unhealthyFingerprint = `app_unhealthy:${attached.app.id}:${row.id}`;
          const probe = await probeAppHttp(conn, token, attached.app);
          if (probe.ok) {
            healthFailCounts.delete(probeKey);
            await resolveAlert(attached.project.organizationId, unhealthyFingerprint, {
              severity: "info",
              code: "app_healthy",
              title: `${attached.app.name} is healthy again`,
              message: `${attached.app.healthCheckPath} returns ${probe.status} on ${row.name}.`,
              serverId: row.id,
              appId: attached.app.id,
              deploymentId: latest.id,
            });
          } else {
            const failures = (healthFailCounts.get(probeKey) ?? 0) + 1;
            healthFailCounts.set(probeKey, failures);
            if (failures >= HEALTH_FAILURE_THRESHOLD) {
              await raiseAlert({
                organizationId: attached.project.organizationId,
                severity: "critical",
                code: "app_unhealthy",
                title: `${attached.app.name} is failing its health check`,
                message: `${attached.app.healthCheckPath} on ${row.name}: ${probe.reason}`,
                fingerprint: unhealthyFingerprint,
                serverId: row.id,
                appId: attached.app.id,
                deploymentId: latest.id,
              });
            }
          }
        }
      }

      const metrics = await getAppMetrics(conn, token, attached.app.id);
      const pressureFingerprint = `resource_pressure:${attached.app.id}:${row.id}`;
      const cpuHot = metrics.cpuPercent >= CPU_PRESSURE_PERCENT;
      const memoryHot = metrics.memoryPercent >= MEMORY_PRESSURE_PERCENT;
      if (cpuHot || memoryHot) {
        const count = (pressureCounts.get(pressureFingerprint) ?? 0) + 1;
        pressureCounts.set(pressureFingerprint, count);
        if (count >= PRESSURE_FAILURE_THRESHOLD) {
          await raiseAlert({
            organizationId: attached.project.organizationId,
            severity: "warning",
            code: "resource_pressure",
            title: `${attached.app.name} is under resource pressure`,
            message: `CPU ${metrics.cpuPercent.toFixed(1)}%, memory ${metrics.memoryPercent.toFixed(1)}% on ${row.name}.`,
            fingerprint: pressureFingerprint,
            serverId: row.id,
            appId: attached.app.id,
            deploymentId: latest.id,
          });
        }
      } else {
        pressureCounts.delete(pressureFingerprint);
        await resolveAlert(attached.project.organizationId, pressureFingerprint, {
          severity: "info",
          code: "resource_pressure_recovered",
          title: `${attached.app.name} resource usage recovered`,
          message: `CPU ${metrics.cpuPercent.toFixed(1)}%, memory ${metrics.memoryPercent.toFixed(1)}% on ${row.name}.`,
          serverId: row.id,
          appId: attached.app.id,
          deploymentId: latest.id,
        });
      }
    } catch (error) {
      await raiseAlert({
        organizationId: attached.project.organizationId,
        severity: "warning",
        code: "app_monitor_failed",
        title: `Could not monitor ${attached.app.name}`,
        message: error instanceof Error ? error.message : String(error),
        fingerprint: `app_monitor_failed:${attached.app.id}:${row.id}`,
        serverId: row.id,
        appId: attached.app.id,
        deploymentId: latest.id,
      });
    }
  }
}

export async function runMonitorOnce(): Promise<void> {
  await checkStuckDeployments();
  await resolveFinishedDeploymentAlerts();

  const servers = await db
    .select()
    .from(server)
    .where(inArray(server.status, ["active", "unreachable", "error"]));

  for (const row of servers) {
    try {
      const reachable = await checkServer(row);
      if (reachable) await checkAppsOnServer(row);
    } catch (error) {
      await raiseAlert({
        organizationId: row.organizationId,
        severity: "warning",
        code: "server_monitor_failed",
        title: `Could not monitor ${row.name}`,
        message: error instanceof Error ? error.message : String(error),
        fingerprint: `server_monitor_failed:${row.id}`,
        serverId: row.id,
      });
    }
  }
}

export function startMonitor(): { close: () => void } {
  let running = false;
  let stopped = false;

  async function tick() {
    if (running || stopped) return;
    running = true;
    try {
      await runMonitorOnce();
    } catch (error) {
      console.error("[monitor]", error instanceof Error ? error.message : error);
    } finally {
      running = false;
    }
  }

  const timer = setInterval(() => void tick(), MONITOR_INTERVAL_MS);
  void tick();

  return {
    close: () => {
      stopped = true;
      clearInterval(timer);
    },
  };
}
