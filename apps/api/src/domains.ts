import { db, domain, server } from "@basse/db";
import type { CreateDomainInput, Domain } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import {
  cloudPreviewReservedHostMessage,
  deleteCloudPreviewDns,
  isCloudPreviewHost,
} from "./cloud-preview";
import { enqueueOrRunDomainSync } from "./proxy-sync";
import { resolveActiveWorkspace } from "./workspace";

type DomainRow = typeof domain.$inferSelect;

// Host/upstream validation mirrors the agent's (caddyx) so bad input is rejected
// before it ever reaches a server. The control plane's own domain is reserved.
const HOST_PATTERN = /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const UPSTREAM_HOST_PATTERN = /^[a-zA-Z0-9](?:[a-zA-Z0-9._-]{0,253}[a-zA-Z0-9])?$/;
const CONTROL_PLANE_DOMAIN = Bun.env.DOMAIN?.trim().toLowerCase();

export function validateHost(host: string): string | null {
  if (!host) return "host is required";
  if (host.includes("*")) return "wildcard hosts are not supported";
  if (host.length > 253 || !HOST_PATTERN.test(host)) return `invalid host: ${host}`;
  if (CONTROL_PLANE_DOMAIN && host === CONTROL_PLANE_DOMAIN) return "this host is reserved";
  return null;
}

export function validateUpstream(upstream: string): string | null {
  const [host, port, ...rest] = upstream.split(":");
  if (rest.length > 0 || !host || port === undefined) return "upstream must be host:port";
  if (!UPSTREAM_HOST_PATTERN.test(host)) return `invalid upstream host: ${host}`;
  const n = Number(port);
  if (!Number.isInteger(n) || n < 1 || n > 65535) return `invalid upstream port: ${port}`;
  return null;
}

function toDomain(row: DomainRow): Domain {
  return {
    id: row.id,
    serverId: row.serverId,
    appId: row.appId,
    host: row.host,
    upstream: row.upstream,
    status: row.status,
    statusMessage: row.statusMessage,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

/** Confirms the server exists and belongs to the active workspace. */
async function ownedServerId(serverId: string, organizationId: string): Promise<string | null> {
  const [row] = await db
    .select({ id: server.id })
    .from(server)
    .where(and(eq(server.id, serverId), eq(server.organizationId, organizationId)))
    .limit(1);
  return row?.id ?? null;
}

export const domains = new Hono();

domains.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const serverId = c.req.query("serverId");
  if (!serverId || !(await ownedServerId(serverId, organizationId))) {
    return c.json({ error: "Server not found" }, 404);
  }

  const rows = await db
    .select()
    .from(domain)
    .where(eq(domain.serverId, serverId))
    .orderBy(domain.createdAt);

  return c.json(rows.map(toDomain));
});

domains.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as
    | (Partial<CreateDomainInput> & { serverId?: unknown })
    | null;
  const serverId = typeof body?.serverId === "string" ? body.serverId : "";
  const host = typeof body?.host === "string" ? body.host.trim().toLowerCase() : "";
  const upstream = typeof body?.upstream === "string" ? body.upstream.trim() : "";
  const appId = typeof body?.appId === "string" && body.appId ? body.appId : null;

  if (!(await ownedServerId(serverId, organizationId))) {
    return c.json({ error: "Server not found" }, 404);
  }

  const hostError = validateHost(host);
  if (hostError) return c.json({ error: hostError }, 400);
  if (isCloudPreviewHost(host)) {
    return c.json({ error: cloudPreviewReservedHostMessage() }, 400);
  }
  const upstreamError = validateUpstream(upstream);
  if (upstreamError) return c.json({ error: upstreamError }, 400);

  const now = new Date();
  let created: DomainRow | undefined;
  try {
    [created] = await db
      .insert(domain)
      .values({
        id: crypto.randomUUID(),
        serverId,
        appId,
        host,
        upstream,
        status: "pending",
        createdAt: now,
        updatedAt: now,
      })
      .returning();
  } catch {
    return c.json({ error: "That host is already in use" }, 409);
  }

  if (!created) {
    return c.json({ error: "Failed to create domain" }, 500);
  }

  // Push the new desired set to the server's proxy in the background, falling
  // back to an inline sync when the queue is unavailable.
  const sync = await enqueueOrRunDomainSync(serverId);
  if (!sync.ok) {
    const updatedAt = new Date();
    await db
      .update(domain)
      .set({ status: "error", statusMessage: sync.error, updatedAt })
      .where(eq(domain.id, created.id))
      .catch(() => {});
    created = { ...created, status: "error", statusMessage: sync.error, updatedAt };
  }

  return c.json(toDomain(created), 201);
});

domains.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  // Join domain->server to enforce ownership (domain has no organizationId).
  const [row] = await db
    .select({ id: domain.id, serverId: domain.serverId, host: domain.host })
    .from(domain)
    .innerJoin(server, eq(domain.serverId, server.id))
    .where(and(eq(domain.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Domain not found" }, 404);
  }

  try {
    await deleteCloudPreviewDns(row.host);
  } catch (error) {
    return c.json(
      { error: error instanceof Error ? error.message : "Failed to delete preview DNS record" },
      502,
    );
  }

  await db.delete(domain).where(eq(domain.id, row.id));

  // Re-push the (now smaller) desired set so the route disappears from Caddy.
  await enqueueOrRunDomainSync(row.serverId);

  return c.body(null, 204);
});
