import { app, appCronJob, db } from "@basse/db";
import type { CreateCronJobInput, CronJob, UpdateCronJobInput } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { execAppCommand } from "./agent-client";
import { ownedApp, requireAgentTarget } from "./apps";
import { cronMatches, validateCronSchedule } from "./cron-schedule";
import { enqueueAction } from "./queue/queue";
import { resolveActiveWorkspace } from "./workspace";

type CronJobRow = typeof appCronJob.$inferSelect;

const SCHEDULER_INTERVAL_MS = 60 * 1000;

export const cronJobs = new Hono();

function toCronJob(row: CronJobRow): CronJob {
  return {
    id: row.id,
    appId: row.appId,
    name: row.name,
    command: row.command,
    schedule: row.schedule,
    enabled: row.enabled,
    lastStatus: row.lastStatus,
    lastRunAt: row.lastRunAt?.toISOString() ?? null,
    lastFinishedAt: row.lastFinishedAt?.toISOString() ?? null,
    lastOutput: row.lastOutput,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

function validateSchedule(schedule: string): string | null {
  return validateCronSchedule(schedule);
}

function validateInput(body: Partial<CreateCronJobInput | UpdateCronJobInput> | null): {
  updates: Partial<CronJobRow>;
  error?: string;
} {
  const updates: Partial<CronJobRow> = {};
  if (typeof body?.name === "string") {
    const name = body.name.trim();
    if (!name) return { updates, error: "name is required" };
    if (name.length > 80) return { updates, error: "name is too long" };
    updates.name = name;
  }
  if (typeof body?.command === "string") {
    const command = body.command.trim();
    if (!command) return { updates, error: "command is required" };
    if (command.length > 500) return { updates, error: "command is too long" };
    updates.command = command;
  }
  if (typeof body?.schedule === "string") {
    const schedule = body.schedule.trim();
    const error = validateSchedule(schedule);
    if (error) return { updates, error };
    updates.schedule = schedule;
  }
  if (typeof body?.enabled === "boolean") updates.enabled = body.enabled;
  return { updates };
}

async function ownedCronJob(
  jobId: string,
  organizationId: string,
): Promise<CronJobRow | null> {
  const [row] = await db.select().from(appCronJob).where(eq(appCronJob.id, jobId)).limit(1);
  if (!row) return null;
  return (await ownedApp(row.appId, organizationId)) ? row : null;
}

cronJobs.get("/:id/cron-jobs", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  if (!(await ownedApp(appId, organizationId))) return c.json({ error: "App not found" }, 404);
  const rows = await db.select().from(appCronJob).where(eq(appCronJob.appId, appId));
  return c.json(rows.map(toCronJob));
});

cronJobs.post("/:id/cron-jobs", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  if (!(await ownedApp(appId, organizationId))) return c.json({ error: "App not found" }, 404);
  const body = (await c.req.json().catch(() => null)) as Partial<CreateCronJobInput> | null;
  const result = validateInput(body);
  if (result.error) return c.json({ error: result.error }, 400);
  if (!result.updates.name || !result.updates.command || !result.updates.schedule) {
    return c.json({ error: "name, command, and schedule are required" }, 400);
  }

  const now = new Date();
  const [created] = await db
    .insert(appCronJob)
    .values({
      id: crypto.randomUUID(),
      appId,
      name: result.updates.name,
      command: result.updates.command,
      schedule: result.updates.schedule,
      enabled: result.updates.enabled ?? true,
      createdAt: now,
      updatedAt: now,
    })
    .returning();
  return c.json(toCronJob(created!), 201);
});

cronJobs.patch("/:id/cron-jobs/:jobId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedCronJob(c.req.param("jobId"), organizationId);
  if (!row || row.appId !== c.req.param("id")) return c.json({ error: "Cron job not found" }, 404);
  const body = (await c.req.json().catch(() => null)) as Partial<UpdateCronJobInput> | null;
  const result = validateInput(body);
  if (result.error) return c.json({ error: result.error }, 400);
  const [updated] = await db
    .update(appCronJob)
    .set({ ...result.updates, updatedAt: new Date() })
    .where(eq(appCronJob.id, row.id))
    .returning();
  return c.json(toCronJob(updated ?? row));
});

cronJobs.delete("/:id/cron-jobs/:jobId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedCronJob(c.req.param("jobId"), organizationId);
  if (!row || row.appId !== c.req.param("id")) return c.json({ error: "Cron job not found" }, 404);
  await db.delete(appCronJob).where(eq(appCronJob.id, row.id));
  return c.body(null, 204);
});

cronJobs.post("/:id/cron-jobs/:jobId/run", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedCronJob(c.req.param("jobId"), organizationId);
  if (!row || row.appId !== c.req.param("id")) return c.json({ error: "Cron job not found" }, 404);
  await enqueueAction("cron-job", row.id);
  return c.json({ ok: true }, 202);
});

export async function runCronJob(jobId: string): Promise<void> {
  const [job] = await db.select().from(appCronJob).where(eq(appCronJob.id, jobId)).limit(1);
  if (!job) return;
  const [appRow] = await db.select().from(app).where(eq(app.id, job.appId)).limit(1);
  if (!appRow) return;

  const now = new Date();
  await db
    .update(appCronJob)
    .set({ lastStatus: "running", lastRunAt: now, lastOutput: null, updatedAt: now })
    .where(eq(appCronJob.id, job.id));

  try {
    const target = await requireAgentTarget(appRow.id);
    if (!target.server) throw new Error(target.error);
    const result = await execAppCommand(target.connection!, target.token!, appRow.id, job.command);
    await db
      .update(appCronJob)
      .set({
        lastStatus: result.exitCode === 0 ? "succeeded" : "failed",
        lastFinishedAt: new Date(),
        lastOutput: result.output.slice(-4000),
        updatedAt: new Date(),
      })
      .where(eq(appCronJob.id, job.id));
  } catch (error) {
    await db
      .update(appCronJob)
      .set({
        lastStatus: "failed",
        lastFinishedAt: new Date(),
        lastOutput: error instanceof Error ? error.message : "Cron job failed",
        updatedAt: new Date(),
      })
      .where(eq(appCronJob.id, job.id));
  }
}

async function runCronSchedulerOnce(now = new Date()): Promise<void> {
  const rows = await db.select().from(appCronJob).where(eq(appCronJob.enabled, true));
  const minuteKey = now.toISOString().slice(0, 16);
  for (const row of rows) {
    if (!cronMatches(row.schedule, now)) continue;
    if (row.lastRunAt?.toISOString().slice(0, 16) === minuteKey) continue;
    await db
      .update(appCronJob)
      .set({ lastRunAt: now, updatedAt: now })
      .where(and(eq(appCronJob.id, row.id), eq(appCronJob.enabled, true)));
    await enqueueAction("cron-job", row.id).catch((error) => {
      console.error("[cron-scheduler]", row.id, error);
    });
  }
}

export function startCronScheduler(): { close: () => void } {
  const timer = setInterval(() => {
    runCronSchedulerOnce().catch((error) => console.error("[cron-scheduler]", error));
  }, SCHEDULER_INTERVAL_MS);
  return { close: () => clearInterval(timer) };
}
