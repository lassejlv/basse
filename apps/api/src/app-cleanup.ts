import { appServer, db, server } from "@basse/db";
import { eq, inArray } from "drizzle-orm";
import { connectionFromServer } from "./server-connection";
import { runScript } from "./ssh";

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

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
      await runScript(
        connection,
        `docker rm -f ${shellQuote(`basse-app-${row.appId}`)} >/dev/null 2>&1 || true`,
        { timeoutMs: 30_000 },
      );
    }),
  );
}
