import { db, domain, server } from "@basse/db";
import type { DesiredDomain } from "@basse/shared";
import { eq } from "drizzle-orm";
import { syncDomains } from "./agent-client";
import { decryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";

/**
 * Pushes the FULL desired domain set for a server to its Caddy proxy (the DB is
 * authoritative; every change re-pushes everything so the proxy self-heals on
 * re-provision). Reflects the outcome on every domain row's status. Never throws
 * — it is called from provisioning and from the queue, both of which must not be
 * taken down by a sync failure.
 */
export async function syncServerDomains(serverId: string): Promise<void> {
  try {
    const [row] = await db.select().from(server).where(eq(server.id, serverId)).limit(1);

    if (!row || !row.agentToken) {
      // Not provisioned yet — nothing to sync against.
      return;
    }

    const token = await decryptSecret(row.agentToken);
    const connection = await connectionFromServer(row);

    const rows = await db
      .select({ host: domain.host, upstream: domain.upstream })
      .from(domain)
      .where(eq(domain.serverId, serverId));

    const desired: DesiredDomain[] = rows.map((d) => ({ host: d.host, upstream: d.upstream }));

    await syncDomains(connection, token, desired);

    await db
      .update(domain)
      .set({ status: "active", statusMessage: null, updatedAt: new Date() })
      .where(eq(domain.serverId, serverId));
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await db
      .update(domain)
      .set({ status: "error", statusMessage: message, updatedAt: new Date() })
      .where(eq(domain.serverId, serverId))
      .catch(() => {});
  }
}
