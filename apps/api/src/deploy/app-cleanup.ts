import { appServer, databaseBackup, db, s3Connection, server } from "@basse/db";
import { and, eq, inArray } from "drizzle-orm";
import { removeApp } from "../infra/agent-client";
import { decryptSecret } from "../lib/crypto";
import { s3ClientForConnection } from "../routes/s3";
import { connectionFromServer } from "../infra/server-connection";

export async function removeAppContainers(appIds: string[]): Promise<void> {
  const ids = [...new Set(appIds)].filter(Boolean);
  if (ids.length === 0) return;

  await removeUploadedBackups(ids);

  const rows = await db
    .select({ appId: appServer.appId, server })
    .from(appServer)
    .innerJoin(server, eq(appServer.serverId, server.id))
    .where(inArray(appServer.appId, ids));

  await Promise.allSettled(
    rows.map(async (row) => {
      if (row.server.status !== "active" || !row.server.agentToken) return;

      const connection = await connectionFromServer(row.server);
      const token = await decryptSecret(row.server.agentToken);
      await removeApp(connection, token, row.appId);
    }),
  );
}

/**
 * Best-effort removal of the apps' uploaded backup objects from S3 before the
 * databaseBackup rows cascade away with the app (server-side dump files vanish
 * with the app's data volume; S3 copies would otherwise linger forever).
 */
async function removeUploadedBackups(appIds: string[]): Promise<void> {
  const uploaded = await db
    .select()
    .from(databaseBackup)
    .where(and(inArray(databaseBackup.appId, appIds), eq(databaseBackup.s3Status, "uploaded")));

  await Promise.allSettled(
    uploaded.map(async (backup) => {
      if (!backup.s3ConnectionId || !backup.s3Key) return;
      const [connectionRow] = await db
        .select()
        .from(s3Connection)
        .where(eq(s3Connection.id, backup.s3ConnectionId))
        .limit(1);
      if (!connectionRow) return;
      const client = await s3ClientForConnection(connectionRow);
      await client.file(backup.s3Key).delete();
    }),
  );
}
