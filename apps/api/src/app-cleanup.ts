import { appServer, db, server } from "@basse/db";
import { eq, inArray } from "drizzle-orm";
import { removeApp } from "./agent-client";
import { decryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";

export async function removeAppContainers(appIds: string[]): Promise<void> {
  const ids = [...new Set(appIds)].filter(Boolean);
  if (ids.length === 0) return;

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
