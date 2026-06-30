import {
  app,
  appServer,
  db,
  domain,
  environment,
  loadBalancer,
  loadBalancerIntegration,
  loadBalancerTarget,
  project,
  server,
} from "@basse/db";
import type {
  CreateLoadBalancerIntegrationInput,
  CreateManagedLoadBalancerInput,
  LoadBalancerIntegration,
  LoadBalancerProvider,
  ManagedLoadBalancer,
  ManagedLoadBalancerTarget,
} from "@basse/shared";
import { and, eq, inArray } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import {
  deleteHetznerLoadBalancer,
  syncHetznerLoadBalancer,
  testHetznerToken,
} from "./hetzner";
import { enqueueAction } from "./queue/queue";
import { resolveActiveWorkspace } from "./workspace";

type IntegrationRow = typeof loadBalancerIntegration.$inferSelect;
type LoadBalancerRow = typeof loadBalancer.$inferSelect;
type TargetRow = typeof loadBalancerTarget.$inferSelect;
type ServerRow = typeof server.$inferSelect;

const PROVIDERS: LoadBalancerProvider[] = ["hetzner", "cloudflare"];
const HOST_PATTERN = /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;
const CONTROL_PLANE_DOMAIN = Bun.env.DOMAIN?.trim().toLowerCase();

export const loadBalancers = new Hono();

loadBalancers.get("/integrations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const rows = await db
    .select()
    .from(loadBalancerIntegration)
    .where(eq(loadBalancerIntegration.organizationId, organizationId))
    .orderBy(loadBalancerIntegration.createdAt);

  return c.json(rows.map(toIntegration));
});

loadBalancers.post("/integrations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as
    | Partial<CreateLoadBalancerIntegrationInput>
    | null;
  const provider = PROVIDERS.includes(body?.provider as LoadBalancerProvider)
    ? (body?.provider as LoadBalancerProvider)
    : null;
  const name = typeof body?.name === "string" && body.name.trim() ? body.name.trim() : "Hetzner";
  const token = typeof body?.token === "string" ? body.token.trim() : "";

  if (!provider) return c.json({ error: "provider is required" }, 400);
  if (provider !== "hetzner") return c.json({ error: "Cloudflare is not wired yet" }, 400);
  if (!token) return c.json({ error: "token is required" }, 400);

  try {
    await testHetznerToken(token);
  } catch (error) {
    return c.json({ error: error instanceof Error ? error.message : "Could not validate token" }, 400);
  }

  const now = new Date();
  const encryptedToken = await encryptSecret(token);
  const tokenHint = token.slice(-4);

  const [existing] = await db
    .select()
    .from(loadBalancerIntegration)
    .where(
      and(
        eq(loadBalancerIntegration.organizationId, organizationId),
        eq(loadBalancerIntegration.provider, provider),
      ),
    )
    .limit(1);

  const [row] = existing
    ? await db
        .update(loadBalancerIntegration)
        .set({
          name,
          token: encryptedToken,
          tokenHint,
          status: "active",
          statusMessage: null,
          updatedAt: now,
        })
        .where(eq(loadBalancerIntegration.id, existing.id))
        .returning()
    : await db
        .insert(loadBalancerIntegration)
        .values({
          id: crypto.randomUUID(),
          organizationId,
          provider,
          name,
          token: encryptedToken,
          tokenHint,
          status: "active",
          createdAt: now,
          updatedAt: now,
        })
        .returning();

  if (!row) return c.json({ error: "Failed to save integration" }, 500);
  return c.json(toIntegration(row), existing ? 200 : 201);
});

loadBalancers.delete("/integrations/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const integrationId = c.req.param("id");
  const integration = await ownedIntegration(integrationId, organizationId);
  if (!integration) return c.json({ error: "Integration not found" }, 404);

  const [existingLoadBalancer] = await db
    .select({ id: loadBalancer.id })
    .from(loadBalancer)
    .where(
      and(
        eq(loadBalancer.integrationId, integrationId),
        eq(loadBalancer.organizationId, organizationId),
      ),
    )
    .limit(1);

  if (existingLoadBalancer) {
    return c.json({ error: "Delete managed load balancers before removing this integration" }, 409);
  }

  await db
    .delete(loadBalancerIntegration)
    .where(
      and(
        eq(loadBalancerIntegration.id, integrationId),
        eq(loadBalancerIntegration.organizationId, organizationId),
      ),
    )
    .returning({ id: loadBalancerIntegration.id });

  return c.body(null, 204);
});

loadBalancers.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const appId = c.req.query("appId");
  if (!appId) return c.json({ error: "appId is required" }, 400);

  const appRow = await ownedApp(appId, organizationId);
  if (!appRow) return c.json({ error: "App not found" }, 404);

  return c.json(await listManagedLoadBalancers(appId, organizationId));
});

loadBalancers.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as
    | Partial<CreateManagedLoadBalancerInput>
    | null;
  const appId = typeof body?.appId === "string" ? body.appId : "";
  const integrationId = typeof body?.integrationId === "string" ? body.integrationId : "";
  const host = typeof body?.host === "string" ? body.host.trim().toLowerCase() : "";
  const location =
    typeof body?.location === "string" && body.location.trim() ? body.location.trim() : "fsn1";
  const loadBalancerType =
    typeof body?.loadBalancerType === "string" && body.loadBalancerType.trim()
      ? body.loadBalancerType.trim()
      : "lb11";
  const healthCheckPath =
    typeof body?.healthCheckPath === "string" && body.healthCheckPath.trim()
      ? normalizeHealthCheckPath(body.healthCheckPath)
      : "/";

  const appRow = await ownedApp(appId, organizationId);
  if (!appRow) return c.json({ error: "App not found" }, 404);
  if (appRow.appKind !== "service") return c.json({ error: "Only service apps can use a load balancer" }, 400);

  const integration = await ownedIntegration(integrationId, organizationId);
  if (!integration) return c.json({ error: "Integration not found" }, 404);
  if (integration.provider !== "hetzner") return c.json({ error: "Provider is not wired yet" }, 400);

  const hostError = validateHost(host);
  if (hostError) return c.json({ error: hostError }, 400);
  const pathError = validateHealthCheckPath(healthCheckPath);
  if (pathError) return c.json({ error: pathError }, 400);

  const now = new Date();
  let row: LoadBalancerRow | undefined;
  try {
    [row] = await db
      .insert(loadBalancer)
      .values({
        id: crypto.randomUUID(),
        organizationId,
        integrationId,
        appId,
        provider: integration.provider,
        name:
          typeof body?.name === "string" && body.name.trim()
            ? body.name.trim()
            : `basse-${appRow.slug}-${appRow.id.slice(0, 8)}`,
        host,
        location,
        loadBalancerType,
        healthCheckPath,
        status: "pending",
        createdAt: now,
        updatedAt: now,
      })
      .returning();
  } catch {
    return c.json({ error: "This app or host already has a managed load balancer" }, 409);
  }

  if (!row) return c.json({ error: "Failed to create load balancer" }, 500);
  const synced = await syncManagedLoadBalancer(row.id, organizationId);
  return c.json(synced, 201);
});

loadBalancers.post("/:id/sync", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedLoadBalancer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Load balancer not found" }, 404);

  return c.json(await syncManagedLoadBalancer(row.id, organizationId));
});

loadBalancers.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedLoadBalancer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Load balancer not found" }, 404);

  const integration = await ownedIntegration(row.integrationId, organizationId);
  if (integration && row.providerResourceId) {
    const token = await decryptSecret(integration.token);
    if (row.provider === "hetzner") {
      await deleteHetznerLoadBalancer(token, row.providerResourceId);
    }
  }

  const targetRows = await db
    .select({ serverId: loadBalancerTarget.serverId })
    .from(loadBalancerTarget)
    .where(eq(loadBalancerTarget.loadBalancerId, row.id));

  await db
    .delete(domain)
    .where(and(eq(domain.appId, row.appId), eq(domain.host, row.host)));
  await db.delete(loadBalancer).where(eq(loadBalancer.id, row.id));
  await enqueueDomainSyncs(targetRows.map((target) => target.serverId));

  return c.body(null, 204);
});

function toIntegration(row: IntegrationRow): LoadBalancerIntegration {
  return {
    id: row.id,
    organizationId: row.organizationId,
    provider: row.provider,
    name: row.name,
    tokenHint: row.tokenHint,
    status: row.status,
    statusMessage: row.statusMessage,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

function toTarget(row: TargetRow): ManagedLoadBalancerTarget {
  return {
    id: row.id,
    serverId: row.serverId,
    address: row.address,
    providerTargetId: row.providerTargetId,
    status: row.status,
    statusMessage: row.statusMessage,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

function toManagedLoadBalancer(
  row: LoadBalancerRow,
  targets: TargetRow[],
): ManagedLoadBalancer {
  return {
    id: row.id,
    organizationId: row.organizationId,
    integrationId: row.integrationId,
    appId: row.appId,
    provider: row.provider,
    name: row.name,
    host: row.host,
    location: row.location,
    loadBalancerType: row.loadBalancerType,
    healthCheckPath: row.healthCheckPath,
    providerResourceId: row.providerResourceId,
    endpointIpv4: row.endpointIpv4,
    endpointIpv6: row.endpointIpv6,
    status: row.status,
    statusMessage: row.statusMessage,
    lastSyncedAt: row.lastSyncedAt?.toISOString() ?? null,
    targets: targets.map(toTarget),
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

async function listManagedLoadBalancers(
  appId: string,
  organizationId: string,
): Promise<ManagedLoadBalancer[]> {
  const rows = await db
    .select()
    .from(loadBalancer)
    .where(and(eq(loadBalancer.appId, appId), eq(loadBalancer.organizationId, organizationId)))
    .orderBy(loadBalancer.createdAt);

  if (rows.length === 0) return [];

  const targetRows = await db
    .select()
    .from(loadBalancerTarget)
    .where(
      inArray(
        loadBalancerTarget.loadBalancerId,
        rows.map((row) => row.id),
      ),
    );
  const targetsByLoadBalancer = new Map<string, TargetRow[]>();
  for (const target of targetRows) {
    const list = targetsByLoadBalancer.get(target.loadBalancerId) ?? [];
    list.push(target);
    targetsByLoadBalancer.set(target.loadBalancerId, list);
  }

  return rows.map((row) => toManagedLoadBalancer(row, targetsByLoadBalancer.get(row.id) ?? []));
}

async function syncManagedLoadBalancer(
  loadBalancerId: string,
  organizationId: string,
): Promise<ManagedLoadBalancer> {
  const row = await ownedLoadBalancer(loadBalancerId, organizationId);
  if (!row) throw new Error("Load balancer not found");

  await db
    .update(loadBalancer)
    .set({ status: "syncing", statusMessage: null, updatedAt: new Date() })
    .where(eq(loadBalancer.id, row.id));

  try {
    const integration = await ownedIntegration(row.integrationId, organizationId);
    if (!integration) throw new Error("Integration not found");

    const appContext = await loadAppContext(row.appId, organizationId);
    if (!appContext) throw new Error("App not found");
    if (appContext.servers.length === 0) {
      throw new Error("Attach at least one server before syncing the load balancer");
    }

    await ensureDomains(row, appContext);

    const token = await decryptSecret(integration.token);
    const syncResult = await syncHetznerLoadBalancer({
      token,
      providerResourceId: row.providerResourceId,
      name: row.name,
      appId: row.appId,
      host: row.host,
      location: row.location,
      loadBalancerType: row.loadBalancerType,
      healthCheckPath: row.healthCheckPath,
      targets: appContext.servers.map((target) => ({
        serverId: target.id,
        name: target.name,
        address: target.sshHost,
      })),
    });

    const now = new Date();
    const staleDomainServerIds: string[] = [];

    await db.transaction(async (tx) => {
      await tx
        .update(loadBalancer)
        .set({
          providerResourceId: syncResult.providerResourceId,
          endpointIpv4: syncResult.endpointIpv4,
          endpointIpv6: syncResult.endpointIpv6,
          status: "active",
          statusMessage: null,
          lastSyncedAt: now,
          updatedAt: now,
        })
        .where(eq(loadBalancer.id, row.id));

      const desiredServerIds = syncResult.targets.map((target) => target.serverId);
      const existingTargets = await tx
        .select()
        .from(loadBalancerTarget)
        .where(eq(loadBalancerTarget.loadBalancerId, row.id));

      const staleTargets = existingTargets.filter(
        (target) => !desiredServerIds.includes(target.serverId),
      );
      for (const target of staleTargets) {
        await tx.delete(loadBalancerTarget).where(eq(loadBalancerTarget.id, target.id));
        await tx
          .delete(domain)
          .where(
            and(
              eq(domain.serverId, target.serverId),
              eq(domain.host, row.host),
              eq(domain.appId, row.appId),
            ),
          );
        staleDomainServerIds.push(target.serverId);
      }

      for (const target of syncResult.targets) {
        const existing = existingTargets.find((candidate) => candidate.serverId === target.serverId);
        if (existing) {
          await tx
            .update(loadBalancerTarget)
            .set({
              address: target.address,
              providerTargetId: target.providerTargetId,
              status: "active",
              statusMessage: null,
              updatedAt: now,
            })
            .where(eq(loadBalancerTarget.id, existing.id));
        } else {
          await tx.insert(loadBalancerTarget).values({
            id: crypto.randomUUID(),
            loadBalancerId: row.id,
            serverId: target.serverId,
            address: target.address,
            providerTargetId: target.providerTargetId,
            status: "active",
            createdAt: now,
            updatedAt: now,
          });
        }
      }
    });

    await enqueueDomainSyncs([
      ...appContext.servers.map((target) => target.id),
      ...staleDomainServerIds,
    ]);
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await db
      .update(loadBalancer)
      .set({ status: "error", statusMessage: message, updatedAt: new Date() })
      .where(eq(loadBalancer.id, row.id));
  }

  const [updated] = await listManagedLoadBalancers(row.appId, organizationId);
  if (!updated) throw new Error("Load balancer not found");
  return updated;
}

async function ensureDomains(
  row: LoadBalancerRow,
  appContext: { app: typeof app.$inferSelect; servers: ServerRow[] },
) {
  const upstream = `basse-app-${row.appId}:${appContext.app.port}`;
  const now = new Date();

  for (const target of appContext.servers) {
    const [existing] = await db
      .select()
      .from(domain)
      .where(and(eq(domain.serverId, target.id), eq(domain.host, row.host)))
      .limit(1);

    if (existing && existing.appId && existing.appId !== row.appId) {
      throw new Error(`${row.host} already routes to another app on ${target.name}`);
    }

    if (existing) {
      await db
        .update(domain)
        .set({
          appId: row.appId,
          upstream,
          status: "pending",
          statusMessage: null,
          updatedAt: now,
        })
        .where(eq(domain.id, existing.id));
    } else {
      await db.insert(domain).values({
        id: crypto.randomUUID(),
        serverId: target.id,
        appId: row.appId,
        host: row.host,
        upstream,
        status: "pending",
        createdAt: now,
        updatedAt: now,
      });
    }
  }
}

async function enqueueDomainSyncs(serverIds: string[]) {
  await Promise.all([...new Set(serverIds)].map((serverId) => enqueueAction("sync-domains", serverId)));
}

async function ownedIntegration(
  integrationId: string,
  organizationId: string,
): Promise<IntegrationRow | null> {
  const [row] = await db
    .select()
    .from(loadBalancerIntegration)
    .where(
      and(
        eq(loadBalancerIntegration.id, integrationId),
        eq(loadBalancerIntegration.organizationId, organizationId),
      ),
    )
    .limit(1);
  return row ?? null;
}

async function ownedLoadBalancer(
  loadBalancerId: string,
  organizationId: string,
): Promise<LoadBalancerRow | null> {
  const [row] = await db
    .select()
    .from(loadBalancer)
    .where(and(eq(loadBalancer.id, loadBalancerId), eq(loadBalancer.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
}

async function ownedApp(appId: string, organizationId: string) {
  const [row] = await db
    .select({ app })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(app.id, appId), eq(project.organizationId, organizationId)))
    .limit(1);

  return row?.app ?? null;
}

async function loadAppContext(appId: string, organizationId: string) {
  const appRow = await ownedApp(appId, organizationId);
  if (!appRow) return null;

  const rows = await db
    .select({ server })
    .from(appServer)
    .innerJoin(server, eq(appServer.serverId, server.id))
    .where(and(eq(appServer.appId, appId), eq(server.organizationId, organizationId)));

  return {
    app: appRow,
    servers: rows.map((row) => row.server),
  };
}

function validateHost(host: string): string | null {
  if (!host) return "host is required";
  if (host.includes("*")) return "wildcard hosts are not supported";
  if (host.length > 253 || !HOST_PATTERN.test(host)) return `invalid host: ${host}`;
  if (CONTROL_PLANE_DOMAIN && host === CONTROL_PLANE_DOMAIN) return "this host is reserved";
  return null;
}

function normalizeHealthCheckPath(path: string): string {
  const value = path.trim();
  return value.startsWith("/") ? value : `/${value}`;
}

function validateHealthCheckPath(path: string): string | null {
  if (!path.startsWith("/")) return "healthCheckPath must start with /";
  if (path.length > 256 || /\s/.test(path)) {
    return "healthCheckPath must be a URL path without spaces";
  }
  return null;
}
