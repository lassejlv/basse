import { db, hetznerConnection } from "@basse/db";
import type { HetznerLocation, HetznerServerType } from "@basse/shared";
import { eq } from "drizzle-orm";
import { decryptSecret } from "../lib/crypto";

type HetznerErrorBody = {
  error?: {
    code?: string;
    message?: string;
  };
};

type HetznerServer = {
  id: number;
  name: string;
  public_net?: {
    ipv4?: { ip?: string | null } | null;
    ipv6?: { ip?: string | null } | null;
  };
};

type HetznerLoadBalancerTarget = {
  type: "server" | "label_selector" | "ip";
  server?: { id: number; ip?: string };
  ip?: { ip: string };
};

type HetznerLoadBalancerService = {
  protocol: "tcp" | "http" | "https";
  listen_port: number;
};

type HetznerLoadBalancer = {
  id: number;
  name: string;
  public_net?: {
    ipv4?: { ip?: string | null } | null;
    ipv6?: { ip?: string | null } | null;
  };
  services?: HetznerLoadBalancerService[];
  targets?: HetznerLoadBalancerTarget[];
};

type HetznerPagination = {
  meta?: {
    pagination?: {
      next_page?: number | null;
    };
  };
};

type SyncTarget = {
  serverId: string;
  name: string;
  address: string;
};

type ResolvedTarget = SyncTarget & {
  hetznerServerId: number;
};

export type HetznerSyncInput = {
  token: string;
  providerResourceId: string | null;
  name: string;
  appId: string;
  host: string;
  location: string;
  loadBalancerType: string;
  healthCheckPath: string;
  targets: SyncTarget[];
};

export type HetznerSyncResult = {
  providerResourceId: string;
  endpointIpv4: string | null;
  endpointIpv6: string | null;
  targets: {
    serverId: string;
    address: string;
    providerTargetId: string;
  }[];
};

const HCLOUD_API = "https://api.hetzner.cloud/v1";

class HetznerClient {
  constructor(private readonly token: string) {}

  async get<T>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  async post<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>("POST", path, body);
  }

  async delete(path: string): Promise<void> {
    await this.request<void>("DELETE", path);
  }

  private async request<T>(method: string, path: string, body?: unknown): Promise<T> {
    const response = await fetch(`${HCLOUD_API}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${this.token}`,
        ...(body ? { "content-type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });

    if (!response.ok) {
      const parsed = (await response.json().catch(() => null)) as HetznerErrorBody | null;
      const message = parsed?.error?.message ?? `request failed with ${response.status}`;
      throw new Error(`Hetzner: ${message}`);
    }

    if (response.status === 204) {
      return undefined as T;
    }

    return response.json() as Promise<T>;
  }
}

export async function testHetznerToken(token: string): Promise<void> {
  const client = new HetznerClient(token);
  await client.get<{ load_balancer_types: unknown[] }>("/load_balancer_types?per_page=1");
}

export async function deleteHetznerLoadBalancer(token: string, providerResourceId: string) {
  const client = new HetznerClient(token);
  await client.delete(`/load_balancers/${encodeURIComponent(providerResourceId)}`);
}

export async function syncHetznerLoadBalancer(input: HetznerSyncInput): Promise<HetznerSyncResult> {
  if (input.targets.length === 0) {
    throw new Error("Attach at least one server before syncing the load balancer");
  }

  const client = new HetznerClient(input.token);
  const resolvedTargets = await resolveTargets(client, input.targets);
  const existingResource = Boolean(input.providerResourceId);
  const loadBalancerId = input.providerResourceId
    ? Number(input.providerResourceId)
    : await createLoadBalancer(client, input, resolvedTargets);

  if (!Number.isSafeInteger(loadBalancerId)) {
    throw new Error("Invalid Hetzner load balancer id");
  }

  let loadBalancer = await getLoadBalancer(client, loadBalancerId);
  if (existingResource) {
    await syncServices(client, loadBalancer, input);
    await syncTargets(client, loadBalancer, resolvedTargets);
    loadBalancer = await getLoadBalancer(client, loadBalancerId);
  }

  return {
    providerResourceId: String(loadBalancer.id),
    endpointIpv4: loadBalancer.public_net?.ipv4?.ip ?? null,
    endpointIpv6: loadBalancer.public_net?.ipv6?.ip ?? null,
    targets: resolvedTargets.map((target) => ({
      serverId: target.serverId,
      address: target.address,
      providerTargetId: String(target.hetznerServerId),
    })),
  };
}

async function createLoadBalancer(
  client: HetznerClient,
  input: HetznerSyncInput,
  targets: ResolvedTarget[],
): Promise<number> {
  const created = await client.post<{
    load_balancer: HetznerLoadBalancer;
  }>("/load_balancers", {
    name: input.name,
    load_balancer_type: input.loadBalancerType,
    location: input.location,
    public_interface: true,
    algorithm: { type: "round_robin" },
    services: [tcpPassthroughService(80, input), tcpPassthroughService(443, input)],
    targets: targets.map((target) => ({
      type: "server",
      server: { id: target.hetznerServerId },
      use_private_ip: false,
    })),
    labels: {
      "basse-managed": "true",
      "basse-app-id": input.appId,
    },
  });

  return created.load_balancer.id;
}

async function getLoadBalancer(
  client: HetznerClient,
  loadBalancerId: number,
): Promise<HetznerLoadBalancer> {
  const response = await client.get<{ load_balancer: HetznerLoadBalancer }>(
    `/load_balancers/${loadBalancerId}`,
  );
  return response.load_balancer;
}

async function listHetznerServers(client: HetznerClient): Promise<HetznerServer[]> {
  const servers: HetznerServer[] = [];
  let page = 1;

  while (page) {
    const response = await client.get<{ servers: HetznerServer[] } & HetznerPagination>(
      `/servers?per_page=50&page=${page}`,
    );
    servers.push(...response.servers);
    page = response.meta?.pagination?.next_page ?? 0;
  }

  return servers;
}

async function resolveTargets(
  client: HetznerClient,
  targets: SyncTarget[],
): Promise<ResolvedTarget[]> {
  const servers = await listHetznerServers(client);
  const byIp = new Map<string, HetznerServer>();

  for (const server of servers) {
    const ipv4 = normalizeAddress(server.public_net?.ipv4?.ip ?? null);
    const ipv6 = normalizeAddress(server.public_net?.ipv6?.ip ?? null);
    if (ipv4) byIp.set(ipv4, server);
    if (ipv6) byIp.set(ipv6, server);
  }

  return targets.map((target) => {
    const matched = byIp.get(normalizeAddress(target.address) ?? "");
    if (!matched) {
      throw new Error(
        `${target.name} (${target.address}) was not found as a Hetzner Cloud server in this project`,
      );
    }

    return {
      ...target,
      hetznerServerId: matched.id,
    };
  });
}

function normalizeAddress(address: string | null): string | null {
  const value = address
    ?.trim()
    .replace(/^\[|\]$/g, "")
    .toLowerCase();
  return value || null;
}

async function syncServices(
  client: HetznerClient,
  loadBalancer: HetznerLoadBalancer,
  input: HetznerSyncInput,
) {
  const existingPorts = new Set(
    (loadBalancer.services ?? []).map((service) => service.listen_port),
  );
  const services = [tcpPassthroughService(80, input), tcpPassthroughService(443, input)];

  for (const service of services) {
    const action = existingPorts.has(service.listen_port) ? "update_service" : "add_service";
    await client.post(`/load_balancers/${loadBalancer.id}/actions/${action}`, service);
  }
}

function tcpPassthroughService(listenPort: 80 | 443, input: HetznerSyncInput) {
  return {
    protocol: "tcp",
    listen_port: listenPort,
    destination_port: listenPort,
    proxyprotocol: false,
    health_check:
      listenPort === 80
        ? {
            protocol: "http",
            port: 80,
            interval: 15,
            timeout: 10,
            retries: 3,
            http: {
              domain: input.host,
              path: input.healthCheckPath,
              status_codes: ["2??", "3??", "4??"],
              tls: false,
            },
          }
        : {
            protocol: "tcp",
            port: 443,
            interval: 15,
            timeout: 10,
            retries: 3,
          },
  };
}

async function syncTargets(
  client: HetznerClient,
  loadBalancer: HetznerLoadBalancer,
  targets: ResolvedTarget[],
) {
  const desiredIds = new Set(targets.map((target) => target.hetznerServerId));
  const currentServerTargets = (loadBalancer.targets ?? []).filter(
    (target) => target.type === "server" && target.server?.id,
  );
  const currentIds = new Set(currentServerTargets.map((target) => target.server!.id));

  for (const target of targets) {
    if (currentIds.has(target.hetznerServerId)) continue;
    await client.post(`/load_balancers/${loadBalancer.id}/actions/add_target`, {
      type: "server",
      server: { id: target.hetznerServerId },
      use_private_ip: false,
    });
  }

  for (const target of currentServerTargets) {
    const serverId = target.server!.id;
    if (desiredIds.has(serverId)) continue;
    await client.post(`/load_balancers/${loadBalancer.id}/actions/remove_target`, {
      type: "server",
      server: { id: serverId },
    });
  }
}

// ---------------------------------------------------------------------------
// Cloud servers: Basse-created Hetzner Cloud machines (workspace connection).
// ---------------------------------------------------------------------------

type HcloudLocation = {
  name: string;
  description: string;
  city: string;
  country: string;
};

type HcloudServerType = {
  name: string;
  description: string;
  cores: number;
  memory: number;
  disk: number;
  architecture: string;
  deprecated: boolean;
  prices: { location: string; price_monthly: { gross: string } }[];
};

type HcloudServer = {
  id: number;
  status: string;
  public_net?: { ipv4?: { ip?: string | null } | null };
};

type HcloudSshKey = {
  id: number;
  public_key: string;
};

export { testHetznerToken as validateHetznerToken };

/** Returns the workspace's decrypted Hetzner Cloud API token, or null. */
export async function getHetznerToken(organizationId: string): Promise<string | null> {
  const [connection] = await db
    .select()
    .from(hetznerConnection)
    .where(eq(hetznerConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) return null;
  return decryptSecret(connection.apiToken);
}

export async function listHetznerLocations(token: string): Promise<HetznerLocation[]> {
  const client = new HetznerClient(token);
  const response = await client.get<{ locations: HcloudLocation[] }>("/locations?per_page=50");
  return response.locations.map((location) => ({
    slug: location.name,
    name: location.description,
    city: location.city,
    country: location.country,
  }));
}

export async function listHetznerServerTypes(token: string): Promise<HetznerServerType[]> {
  const client = new HetznerClient(token);
  const serverTypes: HcloudServerType[] = [];
  let page = 1;

  while (page) {
    const response = await client.get<{ server_types: HcloudServerType[] } & HetznerPagination>(
      `/server_types?per_page=50&page=${page}`,
    );
    serverTypes.push(...response.server_types);
    page = response.meta?.pagination?.next_page ?? 0;
  }

  return serverTypes
    .filter((type) => !type.deprecated)
    .map((type) => ({
      slug: type.name,
      description: type.description,
      cores: type.cores,
      memory: type.memory,
      disk: type.disk,
      architecture: type.architecture,
      prices: type.prices.map((price) => ({
        location: price.location,
        priceMonthly: Number(price.price_monthly.gross),
      })),
    }));
}

/**
 * Registers the public key on the Hetzner project (idempotent on fingerprint)
 * and returns its id. Server creation references keys by id.
 */
async function ensureHetznerSshKey(
  client: HetznerClient,
  name: string,
  publicKey: string,
): Promise<number> {
  try {
    const created = await client.post<{ ssh_key: HcloudSshKey }>("/ssh_keys", {
      name,
      public_key: publicKey,
    });
    return created.ssh_key.id;
  } catch (error) {
    // "SSH key with the same fingerprint already exists" — look up its id.
    if (error instanceof Error && /fingerprint|already exists|uniqueness/i.test(error.message)) {
      const existing = await client.get<{ ssh_keys: HcloudSshKey[] }>("/ssh_keys?per_page=50");
      // Compare key material only (algorithm + base64 body), ignoring comments.
      const keyBody = publicKey.trim().split(/\s+/).slice(0, 2).join(" ");
      const match = existing.ssh_keys.find(
        (key) => key.public_key.trim().split(/\s+/).slice(0, 2).join(" ") === keyBody,
      );
      if (match) return match.id;
    }
    throw error;
  }
}

export type CreateHetznerCloudServerInput = {
  token: string;
  name: string;
  location: string;
  serverType: string;
  sshPublicKey: string;
  serverId: string;
};

/** Creates a Hetzner Cloud server and returns its id. It boots asynchronously. */
export async function createHetznerCloudServer(
  input: CreateHetznerCloudServerInput,
): Promise<string> {
  const client = new HetznerClient(input.token);
  const keyId = await ensureHetznerSshKey(client, `basse-${input.serverId}`, input.sshPublicKey);

  const created = await client.post<{ server: HcloudServer }>("/servers", {
    name: input.name,
    server_type: input.serverType,
    location: input.location,
    image: "ubuntu-24.04",
    ssh_keys: [keyId],
    start_after_create: true,
    labels: {
      "basse-managed": "true",
      "basse-server-id": input.serverId,
    },
  });

  return String(created.server.id);
}

export type HetznerCloudServerState = {
  status: string;
  publicIpv4: string | null;
};

export async function getHetznerCloudServer(
  token: string,
  hetznerServerId: string,
): Promise<HetznerCloudServerState> {
  const client = new HetznerClient(token);
  const response = await client.get<{ server: HcloudServer }>(
    `/servers/${encodeURIComponent(hetznerServerId)}`,
  );
  return {
    status: response.server.status,
    publicIpv4: response.server.public_net?.ipv4?.ip ?? null,
  };
}

export async function deleteHetznerCloudServer(
  token: string,
  hetznerServerId: string,
): Promise<void> {
  const client = new HetznerClient(token);
  await client.delete(`/servers/${encodeURIComponent(hetznerServerId)}`);
}
