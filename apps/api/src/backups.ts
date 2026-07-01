import {
  app,
  appServer,
  databaseBackup,
  db,
  environment,
  project,
  s3Connection,
  server,
} from "@basse/db";
import type {
  DatabaseBackup,
  DatabaseBackupList,
  DatabaseBackupSettings,
  UpdateDatabaseBackupSettingsInput,
} from "@basse/shared";
import { and, desc, eq, inArray } from "drizzle-orm";
import { Hono } from "hono";
import {
  AGENT_PORT,
  type AgentConnection,
  createAgentBackup,
  deleteAgentBackup,
  restoreAgentBackup,
} from "./agent-client";
import { ownedApp, requireAgentTarget } from "./apps";
import { decryptSecret } from "./crypto";
import { raiseAlert, recordEvent, resolveAlert } from "./monitor";
import { enqueueAction } from "./queue/queue";
import { publishForApp } from "./realtime";
import { ownedS3Connection, s3ClientForConnection } from "./s3";
import { connectionFromServer } from "./server-connection";
import { type SshConnection, withTunnel } from "./ssh";
import { resolveActiveWorkspace } from "./workspace";

type AppRow = typeof app.$inferSelect;
type BackupRow = typeof databaseBackup.$inferSelect;

const SCHEDULER_INTERVAL_MS = 5 * 60 * 1000;
const MIN_INTERVAL_HOURS = 1;
const MAX_INTERVAL_HOURS = 168;
const MIN_RETENTION = 1;
const MAX_RETENTION = 50;
const LIST_LIMIT = 50;

function isSshConnection(connection: AgentConnection): connection is SshConnection {
  return !("mode" in connection);
}

/** Mirrors the volume layout in deploy.ts: postgres 18+ mounts /var/lib/postgresql. */
function postgresDataPath(version: string | null): string {
  const major = Number.parseInt(version ?? "18", 10);
  return Number.isFinite(major) && major >= 18 ? "/var/lib/postgresql" : "/var/lib/postgresql/data";
}

/** The pg_dump/pg_restore target for a postgres database app, or null if not one. */
function backupTarget(row: AppRow): { database: string; user: string; dataDir: string } | null {
  if (row.appKind !== "database") return null;
  if ((row.databaseKind ?? "postgres") !== "postgres") return null;
  return {
    database: row.databaseName ?? "postgres",
    user: row.databaseUser ?? "postgres",
    dataDir: postgresDataPath(row.databaseVersion),
  };
}

function toBackup(row: BackupRow): DatabaseBackup {
  return {
    id: row.id,
    appId: row.appId,
    serverId: row.serverId,
    status: row.status,
    trigger: row.trigger,
    sizeBytes: row.sizeBytes,
    error: row.error,
    s3ConnectionId: row.s3ConnectionId,
    s3Status: row.s3Status,
    s3Key: row.s3Key,
    s3Error: row.s3Error,
    s3UploadedAt: row.s3UploadedAt?.toISOString() ?? null,
    startedAt: row.startedAt?.toISOString() ?? null,
    completedAt: row.completedAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
  };
}

function toSettings(row: AppRow): DatabaseBackupSettings {
  return {
    scheduleEnabled: row.backupScheduleEnabled,
    intervalHours: row.backupIntervalHours,
    retention: row.backupRetention,
    s3ConnectionId: row.backupS3ConnectionId,
  };
}

/** Realtime hint so open Backups tabs refetch. Fire-and-forget. */
function publishBackupEvent(appId: string): void {
  void publishForApp(appId, { type: "backup", appId });
}

async function organizationIdForApp(appId: string): Promise<string | null> {
  const [row] = await db
    .select({ organizationId: project.organizationId })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(app.id, appId))
    .limit(1);
  return row?.organizationId ?? null;
}

/**
 * Backup success/failure notifications (coolify.md "Backups" spec) through the
 * shared monitor pipeline: failures raise a deduped alert (+ email), successes
 * resolve it and log an info event. Always best effort — a notification error
 * must never change a backup's outcome.
 */
async function notifyBackupOutcome(input: {
  kind: "backup" | "upload";
  backup: BackupRow;
  appName: string | null;
  ok: boolean;
  message: string;
}): Promise<void> {
  try {
    const organizationId = await organizationIdForApp(input.backup.appId);
    if (!organizationId) return;

    const code = input.kind === "backup" ? "backup_failed" : "backup_upload_failed";
    const fingerprint = `${code}:${input.backup.appId}`;
    const name = input.appName ?? "database";
    const scope = { serverId: input.backup.serverId, appId: input.backup.appId };

    if (!input.ok) {
      await raiseAlert({
        organizationId,
        severity: "warning",
        code,
        title:
          input.kind === "backup"
            ? `Backup failed for ${name}`
            : `Backup upload to S3 failed for ${name}`,
        message: input.message,
        fingerprint,
        ...scope,
      });
      return;
    }

    await resolveAlert(organizationId, fingerprint, {
      severity: "info",
      code: input.kind === "backup" ? "backup_succeeded" : "backup_upload_succeeded",
      title:
        input.kind === "backup" ? `Backup succeeded for ${name}` : `Backup uploaded for ${name}`,
      message: input.message,
      ...scope,
    });
    await recordEvent({
      organizationId,
      severity: "info",
      code: input.kind === "backup" ? "backup_completed" : "backup_uploaded",
      title:
        input.kind === "backup" ? `Backup completed for ${name}` : `Backup uploaded for ${name}`,
      message: input.message,
      fingerprint,
      ...scope,
    });
  } catch (error) {
    console.error("[backup-notify]", input.backup.id, error);
  }
}

async function ownedBackup(backupId: string, appId: string): Promise<BackupRow | null> {
  const [row] = await db
    .select()
    .from(databaseBackup)
    .where(and(eq(databaseBackup.id, backupId), eq(databaseBackup.appId, appId)))
    .limit(1);
  return row ?? null;
}

async function hasActiveBackup(appId: string): Promise<boolean> {
  const [row] = await db
    .select({ id: databaseBackup.id })
    .from(databaseBackup)
    .where(
      and(eq(databaseBackup.appId, appId), inArray(databaseBackup.status, ["queued", "running"])),
    )
    .limit(1);
  return Boolean(row);
}

export const backups = new Hono();

backups.get("/:id/backups", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  if (!backupTarget(row)) return c.json({ error: "Backups require a postgres database" }, 400);

  const rows = await db
    .select()
    .from(databaseBackup)
    .where(eq(databaseBackup.appId, appId))
    .orderBy(desc(databaseBackup.createdAt))
    .limit(LIST_LIMIT);

  return c.json({
    backups: rows.map(toBackup),
    settings: toSettings(row),
  } satisfies DatabaseBackupList);
});

backups.post("/:id/backups", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  if (!backupTarget(row)) return c.json({ error: "Backups require a postgres database" }, 400);
  if (await hasActiveBackup(appId)) {
    return c.json({ error: "A backup is already in progress" }, 409);
  }

  const target = await requireAgentTarget(appId);
  if (!target.server) return c.json({ error: target.error }, 400);

  const now = new Date();
  const [created] = await db
    .insert(databaseBackup)
    .values({
      id: crypto.randomUUID(),
      appId,
      serverId: target.server.id,
      status: "queued",
      trigger: "manual",
      createdAt: now,
      updatedAt: now,
    })
    .returning();
  if (!created) return c.json({ error: "Failed to create backup" }, 500);

  try {
    await enqueueAction("database-backup", created.id);
  } catch {
    await db.delete(databaseBackup).where(eq(databaseBackup.id, created.id));
    return c.json({ error: "Backup queue is unavailable" }, 503);
  }

  publishBackupEvent(appId);
  return c.json(toBackup(created), 202);
});

backups.post("/:id/backups/:backupId/restore", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  const target = backupTarget(row);
  if (!target) return c.json({ error: "Backups require a postgres database" }, 400);

  const backup = await ownedBackup(c.req.param("backupId"), appId);
  if (!backup) return c.json({ error: "Backup not found" }, 404);
  if (backup.status !== "completed") {
    return c.json({ error: "Only completed backups can be restored" }, 400);
  }

  const agent = await requireAgentTarget(appId, backup.serverId);
  if (!agent.server) return c.json({ error: agent.error }, 400);

  try {
    await restoreAgentBackup(agent.connection!, agent.token!, appId, {
      backupId: backup.id,
      ...target,
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : "Restore failed";
    return c.json({ error: message }, 502);
  }

  return c.json({ ok: true });
});

backups.delete("/:id/backups/:backupId", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);

  const backup = await ownedBackup(c.req.param("backupId"), appId);
  if (!backup) return c.json({ error: "Backup not found" }, 404);
  if (backup.status === "running") {
    return c.json({ error: "Cannot delete a backup that is running" }, 400);
  }

  // Best effort: the server (or its dump file) may already be gone; the DB row
  // is the source of truth for the UI either way.
  const target = backupTarget(row);
  if (target && backup.status === "completed") {
    try {
      const agent = await requireAgentTarget(appId, backup.serverId);
      if (agent.server) {
        await deleteAgentBackup(agent.connection!, agent.token!, appId, backup.id, target.dataDir);
      }
    } catch {
      // ignore — row deletion below is what the user asked for
    }
  }

  await deleteS3Object(backup);
  await db.delete(databaseBackup).where(eq(databaseBackup.id, backup.id));
  return c.json({ ok: true });
});

backups.get("/:id/backups/:backupId/download", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  const target = backupTarget(row);
  if (!target) return c.json({ error: "Backups require a postgres database" }, 400);

  const backup = await ownedBackup(c.req.param("backupId"), appId);
  if (!backup) return c.json({ error: "Backup not found" }, 404);
  if (backup.status !== "completed") {
    return c.json({ error: "Backup is not completed" }, 400);
  }

  const timestamp = backup.createdAt.toISOString().replaceAll(":", "-").slice(0, 19);
  const filename = `${row.slug}-${timestamp}.dump`;

  // Serve from S3 when the copy is there. Streams the uploaded object when the
  // server copy is unreachable (server gone, outbound mode, file pruned).
  const s3Fallback = async (): Promise<Response | null> => {
    if (backup.s3Status !== "uploaded" || !backup.s3Key || !backup.s3ConnectionId) return null;
    const [connectionRow] = await db
      .select()
      .from(s3Connection)
      .where(eq(s3Connection.id, backup.s3ConnectionId))
      .limit(1);
    if (!connectionRow) return null;
    try {
      const client = await s3ClientForConnection(connectionRow);
      const file = client.file(backup.s3Key);
      if (!(await file.exists())) return null;
      c.header("content-type", "application/octet-stream");
      c.header("content-disposition", `attachment; filename="${filename}"`);
      return c.body(file.stream());
    } catch {
      return null;
    }
  };

  const agent = await requireAgentTarget(appId, backup.serverId);
  if (!agent.server || !isSshConnection(agent.connection!)) {
    const fromS3 = await s3Fallback();
    if (fromS3) return fromS3;
    if (!agent.server) return c.json({ error: agent.error }, 400);
    return c.json({ error: "Backup downloads are not supported on outbound servers yet" }, 400);
  }

  // Stream through the SSH tunnel without buffering the dump in memory. The
  // tunnel promise intentionally floats: it resolves only once the upstream
  // body has been fully piped into the response stream.
  const stream = new TransformStream<Uint8Array, Uint8Array>();
  let resolveHead!: (status: number) => void;
  let rejectHead!: (error: unknown) => void;
  const head = new Promise<number>((resolve, reject) => {
    resolveHead = resolve;
    rejectHead = reject;
  });

  const path = `/v1/apps/${appId}/backups/${backup.id}/download?dataDir=${encodeURIComponent(target.dataDir)}`;
  const pump = withTunnel(agent.connection!, AGENT_PORT, async (baseUrl) => {
    const upstream = await fetch(`${baseUrl}${path}`, {
      headers: { authorization: `Bearer ${agent.token!}` },
    });
    if (!upstream.ok || !upstream.body) {
      resolveHead(upstream.status);
      return;
    }
    resolveHead(200);
    await upstream.body.pipeTo(stream.writable);
  });
  pump.catch((error) => {
    rejectHead(error);
    // If piping already started, pipeTo has aborted the writable itself.
    stream.writable.abort(error).catch(() => {});
  });

  let status: number;
  try {
    status = await head;
  } catch {
    const fromS3 = await s3Fallback();
    if (fromS3) return fromS3;
    return c.json({ error: "Could not reach the server" }, 502);
  }
  if (status !== 200) {
    const fromS3 = await s3Fallback();
    if (fromS3) return fromS3;
    return c.json(
      { error: status === 404 ? "Backup file not found on server" : "Download failed" },
      502,
    );
  }

  c.header("content-type", "application/octet-stream");
  c.header("content-disposition", `attachment; filename="${filename}"`);
  return c.body(stream.readable);
});

backups.post("/:id/backups/:backupId/upload", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  if (!backupTarget(row)) return c.json({ error: "Backups require a postgres database" }, 400);
  if (!row.backupS3ConnectionId) {
    return c.json({ error: "Choose an S3 connection in the backup settings first" }, 400);
  }
  const connection = await ownedS3Connection(row.backupS3ConnectionId, organizationId);
  if (!connection) return c.json({ error: "S3 connection not found" }, 404);

  const backup = await ownedBackup(c.req.param("backupId"), appId);
  if (!backup) return c.json({ error: "Backup not found" }, 404);
  if (backup.status !== "completed") {
    return c.json({ error: "Only completed backups can be uploaded" }, 400);
  }
  if (backup.s3Status === "uploading") {
    return c.json({ error: "Upload already in progress" }, 409);
  }

  const [updated] = await db
    .update(databaseBackup)
    .set({
      s3ConnectionId: connection.id,
      s3Status: "uploading",
      s3Error: null,
      updatedAt: new Date(),
    })
    .where(eq(databaseBackup.id, backup.id))
    .returning();

  try {
    await enqueueAction("database-backup-upload", backup.id);
  } catch {
    await db
      .update(databaseBackup)
      .set({ s3Status: backup.s3Status, s3Error: backup.s3Error, updatedAt: new Date() })
      .where(eq(databaseBackup.id, backup.id));
    return c.json({ error: "Backup queue is unavailable" }, 503);
  }

  return c.json(toBackup(updated ?? backup), 202);
});

backups.patch("/:id/backups/settings", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.param("id");
  const row = await ownedApp(appId, organizationId);
  if (!row) return c.json({ error: "App not found" }, 404);
  if (!backupTarget(row)) return c.json({ error: "Backups require a postgres database" }, 400);

  const body = (await c.req.json().catch(() => null)) as UpdateDatabaseBackupSettingsInput | null;
  if (!body) return c.json({ error: "Invalid request body" }, 400);

  const updates: Partial<AppRow> = {};
  if (typeof body.scheduleEnabled === "boolean") {
    updates.backupScheduleEnabled = body.scheduleEnabled;
  }
  if (typeof body.intervalHours !== "undefined") {
    const hours = Number(body.intervalHours);
    if (!Number.isInteger(hours) || hours < MIN_INTERVAL_HOURS || hours > MAX_INTERVAL_HOURS) {
      return c.json(
        { error: `intervalHours must be ${MIN_INTERVAL_HOURS}-${MAX_INTERVAL_HOURS}` },
        400,
      );
    }
    updates.backupIntervalHours = hours;
  }
  if (typeof body.retention !== "undefined") {
    const retention = Number(body.retention);
    if (!Number.isInteger(retention) || retention < MIN_RETENTION || retention > MAX_RETENTION) {
      return c.json({ error: `retention must be ${MIN_RETENTION}-${MAX_RETENTION}` }, 400);
    }
    updates.backupRetention = retention;
  }
  if (typeof body.s3ConnectionId !== "undefined") {
    if (body.s3ConnectionId === null) {
      updates.backupS3ConnectionId = null;
    } else if (typeof body.s3ConnectionId === "string") {
      const connection = await ownedS3Connection(body.s3ConnectionId, organizationId);
      if (!connection) return c.json({ error: "S3 connection not found" }, 404);
      updates.backupS3ConnectionId = connection.id;
    } else {
      return c.json({ error: "Invalid s3ConnectionId" }, 400);
    }
  }
  if (Object.keys(updates).length === 0) return c.json(toSettings(row));

  const [updated] = await db
    .update(app)
    .set({ ...updates, updatedAt: new Date() })
    .where(eq(app.id, appId))
    .returning();

  return c.json(toSettings(updated ?? row));
});

/**
 * Worker handler for the "database-backup" action. Runs pg_dump on the target
 * server via the agent, records size/status on the backup row, and prunes
 * completed backups past the app's retention. Never throws.
 */
export async function runDatabaseBackup(backupId: string): Promise<void> {
  const [backup] = await db
    .select()
    .from(databaseBackup)
    .where(eq(databaseBackup.id, backupId))
    .limit(1);
  if (!backup || backup.status !== "queued") return;

  let appName: string | null = null;
  const fail = async (message: string) => {
    await db
      .update(databaseBackup)
      .set({ status: "failed", error: message, completedAt: new Date(), updatedAt: new Date() })
      .where(eq(databaseBackup.id, backupId));
    publishBackupEvent(backup.appId);
    await notifyBackupOutcome({ kind: "backup", backup, appName, ok: false, message });
  };

  try {
    const [appRow] = await db.select().from(app).where(eq(app.id, backup.appId)).limit(1);
    if (!appRow) return fail("App no longer exists");
    appName = appRow.name;
    const target = backupTarget(appRow);
    if (!target) return fail("App is not a postgres database");

    const [serverRow] = await db
      .select()
      .from(server)
      .where(eq(server.id, backup.serverId))
      .limit(1);
    if (!serverRow) return fail("Server no longer exists");
    if (serverRow.status !== "active" || !serverRow.agentToken) {
      return fail("Server is not active");
    }

    await db
      .update(databaseBackup)
      .set({ status: "running", startedAt: new Date(), updatedAt: new Date() })
      .where(eq(databaseBackup.id, backupId));
    publishBackupEvent(backup.appId);

    const connection = await connectionFromServer(serverRow);
    const token = await decryptSecret(serverRow.agentToken);
    const result = await createAgentBackup(connection, token, appRow.id, {
      backupId: backup.id,
      ...target,
    });

    await db
      .update(databaseBackup)
      .set({
        status: "completed",
        sizeBytes: result.sizeBytes,
        completedAt: new Date(),
        updatedAt: new Date(),
      })
      .where(eq(databaseBackup.id, backupId));
    publishBackupEvent(backup.appId);

    await notifyBackupOutcome({
      kind: "backup",
      backup,
      appName,
      ok: true,
      message: `pg_dump finished (${result.sizeBytes} bytes).`,
    });

    // Auto-offload to S3 when the app has a destination configured.
    if (appRow.backupS3ConnectionId) {
      await db
        .update(databaseBackup)
        .set({
          s3ConnectionId: appRow.backupS3ConnectionId,
          s3Status: "uploading",
          updatedAt: new Date(),
        })
        .where(eq(databaseBackup.id, backupId));
      try {
        await enqueueAction("database-backup-upload", backupId);
      } catch (error) {
        await db
          .update(databaseBackup)
          .set({
            s3Status: "failed",
            s3Error: error instanceof Error ? error.message : "Could not queue upload",
            updatedAt: new Date(),
          })
          .where(eq(databaseBackup.id, backupId));
      }
    }

    await enforceRetention(appRow, connection, token, target.dataDir);
  } catch (error) {
    await fail(error instanceof Error ? error.message : "Backup failed");
  }
}

/**
 * Worker handler for the "database-backup-upload" action. Streams a completed
 * dump from the server (via SSH tunnel + agent) into the configured S3 bucket
 * using Bun's multipart S3 writer, so the dump never buffers fully in memory.
 * Never throws.
 */
export async function runBackupUpload(backupId: string): Promise<void> {
  const [backup] = await db
    .select()
    .from(databaseBackup)
    .where(eq(databaseBackup.id, backupId))
    .limit(1);
  if (!backup || backup.status !== "completed" || backup.s3Status !== "uploading") return;

  let appName: string | null = null;
  const fail = async (message: string) => {
    await db
      .update(databaseBackup)
      .set({ s3Status: "failed", s3Error: message, updatedAt: new Date() })
      .where(eq(databaseBackup.id, backupId));
    publishBackupEvent(backup.appId);
    await notifyBackupOutcome({ kind: "upload", backup, appName, ok: false, message });
  };

  try {
    if (!backup.s3ConnectionId) return fail("No S3 connection configured");
    const [connectionRow] = await db
      .select()
      .from(s3Connection)
      .where(eq(s3Connection.id, backup.s3ConnectionId))
      .limit(1);
    if (!connectionRow) return fail("S3 connection no longer exists");

    const [appRow] = await db.select().from(app).where(eq(app.id, backup.appId)).limit(1);
    if (!appRow) return fail("App no longer exists");
    appName = appRow.name;
    const target = backupTarget(appRow);
    if (!target) return fail("App is not a postgres database");

    const [serverRow] = await db
      .select()
      .from(server)
      .where(eq(server.id, backup.serverId))
      .limit(1);
    if (!serverRow || serverRow.status !== "active" || !serverRow.agentToken) {
      return fail("Server is not active");
    }

    const agentConnection = await connectionFromServer(serverRow);
    if (!isSshConnection(agentConnection)) {
      return fail("S3 uploads are not supported on outbound servers yet");
    }
    const token = await decryptSecret(serverRow.agentToken);

    const client = await s3ClientForConnection(connectionRow);
    const key = `basse-backups/${appRow.slug}/${backup.createdAt.toISOString().slice(0, 19).replaceAll(":", "-")}-${backup.id.slice(0, 8)}.dump`;
    const path = `/v1/apps/${appRow.id}/backups/${backup.id}/download?dataDir=${encodeURIComponent(target.dataDir)}`;

    await withTunnel(agentConnection, AGENT_PORT, async (baseUrl) => {
      const upstream = await fetch(`${baseUrl}${path}`, {
        headers: { authorization: `Bearer ${token}` },
        signal: AbortSignal.timeout(30 * 60 * 1000),
      });
      if (!upstream.ok || !upstream.body) {
        throw new Error(`Could not read dump from server (${upstream.status})`);
      }
      const writer = client.file(key).writer();
      try {
        for await (const chunk of upstream.body) {
          const pending = writer.write(chunk);
          if (pending instanceof Promise) await pending;
        }
        await writer.end();
      } catch (error) {
        await Promise.resolve(writer.end()).catch(() => {});
        throw error;
      }
    });

    await db
      .update(databaseBackup)
      .set({
        s3Status: "uploaded",
        s3Key: key,
        s3Error: null,
        s3UploadedAt: new Date(),
        updatedAt: new Date(),
      })
      .where(eq(databaseBackup.id, backupId));
    publishBackupEvent(backup.appId);

    await notifyBackupOutcome({
      kind: "upload",
      backup,
      appName,
      ok: true,
      message: `Uploaded to ${connectionRow.bucket}/${key}.`,
    });
  } catch (error) {
    await fail(error instanceof Error ? error.message : "Upload failed");
  }
}

/** Best-effort removal of a backup's uploaded S3 object. */
async function deleteS3Object(backup: BackupRow): Promise<void> {
  if (!backup.s3ConnectionId || !backup.s3Key || backup.s3Status !== "uploaded") return;
  try {
    const [connectionRow] = await db
      .select()
      .from(s3Connection)
      .where(eq(s3Connection.id, backup.s3ConnectionId))
      .limit(1);
    if (!connectionRow) return;
    const client = await s3ClientForConnection(connectionRow);
    await client.file(backup.s3Key).delete();
  } catch {
    // Object cleanup is best effort.
  }
}

/** Deletes completed backups beyond the app's retention count (oldest first). */
async function enforceRetention(
  appRow: AppRow,
  connection: AgentConnection,
  token: string,
  dataDir: string,
): Promise<void> {
  const excess = await db
    .select()
    .from(databaseBackup)
    .where(and(eq(databaseBackup.appId, appRow.id), eq(databaseBackup.status, "completed")))
    .orderBy(desc(databaseBackup.createdAt))
    .offset(Math.max(appRow.backupRetention, MIN_RETENTION));

  for (const old of excess) {
    try {
      await deleteAgentBackup(connection, token, appRow.id, old.id, dataDir);
    } catch {
      // File cleanup is best effort; still drop the row so retention converges.
    }
    await deleteS3Object(old);
    await db.delete(databaseBackup).where(eq(databaseBackup.id, old.id));
  }
}

/** Enqueues one backup for a scheduled app whose latest backup is older than its interval. */
async function runSchedulerOnce(): Promise<void> {
  const candidates = await db
    .select()
    .from(app)
    .where(and(eq(app.backupScheduleEnabled, true), eq(app.appKind, "database")));

  for (const appRow of candidates) {
    try {
      if (!backupTarget(appRow)) continue;
      if (await hasActiveBackup(appRow.id)) continue;

      // Pace from the latest attempt regardless of outcome so a failing
      // database retries on its schedule instead of every scheduler tick.
      const [latest] = await db
        .select({ createdAt: databaseBackup.createdAt })
        .from(databaseBackup)
        .where(eq(databaseBackup.appId, appRow.id))
        .orderBy(desc(databaseBackup.createdAt))
        .limit(1);
      const dueAt = latest
        ? latest.createdAt.getTime() + appRow.backupIntervalHours * 60 * 60 * 1000
        : 0;
      if (Date.now() < dueAt) continue;

      const serverId = appRow.serverId ?? (await firstAttachedServerId(appRow.id));
      if (!serverId) continue;

      const now = new Date();
      const [created] = await db
        .insert(databaseBackup)
        .values({
          id: crypto.randomUUID(),
          appId: appRow.id,
          serverId,
          status: "queued",
          trigger: "scheduled",
          createdAt: now,
          updatedAt: now,
        })
        .returning();
      if (created) {
        await enqueueAction("database-backup", created.id);
        publishBackupEvent(appRow.id);
      }
    } catch (error) {
      console.error("[backup-scheduler]", appRow.id, error);
    }
  }
}

async function firstAttachedServerId(appId: string): Promise<string | null> {
  const [row] = await db
    .select({ serverId: appServer.serverId })
    .from(appServer)
    .where(eq(appServer.appId, appId))
    .limit(1);
  return row?.serverId ?? null;
}

/** Periodic scheduler for automatic backups. Mirrors startMonitor's shape. */
export function startBackupScheduler(): { close: () => void } {
  const timer = setInterval(() => {
    runSchedulerOnce().catch((error) => console.error("[backup-scheduler]", error));
  }, SCHEDULER_INTERVAL_MS);
  return {
    close: () => clearInterval(timer),
  };
}
