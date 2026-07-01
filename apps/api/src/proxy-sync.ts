import { db, domain, server } from "@basse/db";
import type { DesiredDomain } from "@basse/shared";
import { eq } from "drizzle-orm";
import { syncDomains } from "./agent-client";
import { decryptSecret } from "./crypto";
import { enqueueAction } from "./queue/queue";
import { connectionFromServer } from "./server-connection";

export type DomainSyncResult =
  | { ok: true; count: number }
  | { ok: false; error: string };
export type DomainSyncQueueResult = { ok: true; queued: true } | DomainSyncResult;

const serverSyncLocks = new Map<string, Promise<void>>();

export async function enqueueOrRunDomainSync(serverId: string): Promise<DomainSyncQueueResult> {
  try {
    await enqueueAction("sync-domains", serverId);
    return { ok: true, queued: true };
  } catch {
    return syncServerDomains(serverId);
  }
}

/**
 * Pushes the FULL desired domain set for a server to its Caddy proxy (the DB is
 * authoritative; every change re-pushes everything so the proxy self-heals on
 * re-provision). Reflects the outcome on every domain row's status. Never throws
 * — it is called from provisioning and from the queue, both of which must not be
 * taken down by a sync failure.
 */
export async function syncServerDomains(serverId: string): Promise<DomainSyncResult> {
  const previous = serverSyncLocks.get(serverId) ?? Promise.resolve();
  let release: () => void = () => {};
  const current = new Promise<void>((resolve) => {
    release = resolve;
  });
  const tail = previous.catch(() => {}).then(() => current);
  serverSyncLocks.set(serverId, tail);

  await previous.catch(() => {});
  try {
    return await syncServerDomainsNow(serverId);
  } finally {
    release();
    if (serverSyncLocks.get(serverId) === tail) {
      serverSyncLocks.delete(serverId);
    }
  }
}

async function syncServerDomainsNow(serverId: string): Promise<DomainSyncResult> {
  try {
    const [row] = await db.select().from(server).where(eq(server.id, serverId)).limit(1);

    if (!row || !row.agentToken) {
      // Not provisioned yet — nothing to sync against.
      return { ok: false, error: "Server is not provisioned" };
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
    return { ok: true, count: desired.length };
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await db
      .update(domain)
      .set({ status: "error", statusMessage: message, updatedAt: new Date() })
      .where(eq(domain.serverId, serverId))
      .catch(() => {});
    return { ok: false, error: message };
  }
}
