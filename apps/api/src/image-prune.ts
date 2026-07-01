import { app, appServer, db, deployment, server, workspaceSettings } from "@basse/db";
import { and, eq, gt, isNotNull, or } from "drizzle-orm";
import { pruneServerImages } from "./agent-client";
import { decryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";

// Enforces workspace imageRetentionDays on user servers. Server builds tag
// basse-app:<deploymentId> and Depot deploys pull registry images; without
// pruning they accumulate until the disk fills. The keep set protects every
// image a user could still roll back to within the retention window, plus the
// currently healthy deployment of each app regardless of age.

const PRUNE_INTERVAL_MS = 6 * 60 * 60 * 1000;
const FIRST_RUN_DELAY_MS = 15 * 60 * 1000;
const DEFAULT_RETENTION_DAYS = 30;

async function keepRefsForServer(serverId: string, cutoff: Date): Promise<string[]> {
  const rows = await db
    .select({ imageRef: deployment.imageRef })
    .from(deployment)
    .innerJoin(app, eq(deployment.appId, app.id))
    .innerJoin(appServer, eq(appServer.appId, app.id))
    .where(
      and(
        eq(appServer.serverId, serverId),
        isNotNull(deployment.imageRef),
        or(eq(deployment.status, "healthy"), gt(deployment.createdAt, cutoff)),
      ),
    );
  return [...new Set(rows.map((row) => row.imageRef!))];
}

async function pruneServer(row: typeof server.$inferSelect): Promise<void> {
  const [settings] = await db
    .select({ imageRetentionDays: workspaceSettings.imageRetentionDays })
    .from(workspaceSettings)
    .where(eq(workspaceSettings.organizationId, row.organizationId))
    .limit(1);
  const retentionDays = settings?.imageRetentionDays ?? DEFAULT_RETENTION_DAYS;
  const cutoff = new Date(Date.now() - retentionDays * 24 * 60 * 60 * 1000);

  const keepRefs = await keepRefsForServer(row.id, cutoff);
  const connection = await connectionFromServer(row);
  const token = await decryptSecret(row.agentToken!);
  const result = await pruneServerImages(connection, token, {
    keepRefs,
    olderThanHours: retentionDays * 24,
  });
  if (result.removed > 0) {
    console.log(
      `[image-prune] ${row.name}: removed ${result.removed} image(s), skipped ${result.skipped}`,
    );
  }
}

export async function runImagePruneOnce(): Promise<void> {
  const rows = await db
    .select()
    .from(server)
    .where(and(eq(server.status, "active"), isNotNull(server.agentToken)));

  for (const row of rows) {
    try {
      await pruneServer(row);
    } catch (error) {
      console.error("[image-prune]", row.id, error instanceof Error ? error.message : error);
    }
  }
}

/** Periodic image retention enforcement. Mirrors startMonitor's shape. */
export function startImagePruner(): { close: () => void } {
  const timers: ReturnType<typeof setTimeout>[] = [];
  const first = setTimeout(() => {
    void runImagePruneOnce().catch((error) => console.error("[image-prune]", error));
  }, FIRST_RUN_DELAY_MS);
  const recurring = setInterval(() => {
    void runImagePruneOnce().catch((error) => console.error("[image-prune]", error));
  }, PRUNE_INTERVAL_MS);
  timers.push(first, recurring);
  return {
    close: () => {
      clearTimeout(first);
      clearInterval(recurring);
    },
  };
}
