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
