type CloudflareError = {
  code?: number;
  message?: string;
};

type CloudflareEnvelope<T> = {
  success?: boolean;
  errors?: CloudflareError[];
  messages?: CloudflareError[];
  result: T;
  result_info?: {
    page?: number;
    per_page?: number;
    total_pages?: number;
  };
};

type CloudflareZone = {
  id: string;
  name: string;
  account?: {
    id?: string;
    name?: string;
  };
};

type CloudflareMonitor = {
  id: string;
};

type CloudflarePool = {
  id: string;
};

type CloudflareLoadBalancer = {
  id: string;
};

type CloudflareDnsRecord = {
  id: string;
  name: string;
  type: string;
  content: string;
};

type SyncTarget = {
  serverId: string;
  name: string;
  address: string;
};

type CloudflareResourceRef = {
  zoneId: string;
  accountId: string;
  loadBalancerId: string;
  poolId: string;
  monitorId: string;
};

export type CloudflareSyncInput = {
  token: string;
  providerResourceId: string | null;
  name: string;
  appId: string;
  host: string;
  healthCheckPath: string;
  targets: SyncTarget[];
};

export type CloudflareSyncResult = {
  providerResourceId: string;
  endpointIpv4: null;
  endpointIpv6: null;
  targets: {
    serverId: string;
    address: string;
    providerTargetId: string;
  }[];
};

const CLOUDFLARE_API = "https://api.cloudflare.com/client/v4";
const CLOUDFLARE_TOKEN_HELP =
  "Use a Cloudflare API token with Zone Read, Zone Load Balancers Edit, and Account Load Balancing Monitors and Pools Edit permissions for the matching zone/account.";

class CloudflareClient {
  constructor(private readonly token: string) {}

  async get<T>(path: string): Promise<T> {
    return this.request<T>("GET", path);
  }

  async post<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>("POST", path, body);
  }

  async put<T>(path: string, body: unknown): Promise<T> {
    return this.request<T>("PUT", path, body);
  }

  async delete(path: string): Promise<void> {
    await this.request<unknown>("DELETE", path, undefined, { ignoreNotFound: true });
  }

  async envelope<T>(path: string): Promise<CloudflareEnvelope<T>> {
    return this.rawRequest<T>("GET", path);
  }

  private async request<T>(
    method: string,
    path: string,
    body?: unknown,
    options: { ignoreNotFound?: boolean } = {},
  ): Promise<T> {
    const parsed = await this.rawRequest<T>(method, path, body, options);
    return parsed.result;
  }

  private async rawRequest<T>(
    method: string,
    path: string,
    body?: unknown,
    options: { ignoreNotFound?: boolean } = {},
  ): Promise<CloudflareEnvelope<T>> {
    const response = await fetch(`${CLOUDFLARE_API}${path}`, {
      method,
      headers: {
        authorization: `Bearer ${this.token}`,
        ...(body ? { "content-type": "application/json" } : {}),
      },
      body: body ? JSON.stringify(body) : undefined,
    });

    if (options.ignoreNotFound && response.status === 404) {
      return { result: undefined as T };
    }

    const parsed = (await response.json().catch(() => null)) as CloudflareEnvelope<T> | null;
    if (!response.ok || parsed?.success === false) {
      const message =
        parsed?.errors?.map((error) => error.message).filter(Boolean).join(", ") ||
        `request failed with ${response.status}`;
      const authFailed =
        response.status === 401 ||
        response.status === 403 ||
        message.toLowerCase().includes("authentication");
      if (authFailed) {
        throw new Error(`Cloudflare: ${message}. ${CLOUDFLARE_TOKEN_HELP}`);
      }
      throw new Error(`Cloudflare: ${message}`);
    }

    if (!parsed) {
      return { result: undefined as T };
    }

    return parsed;
  }
}

export async function testCloudflareToken(token: string): Promise<void> {
  const client = new CloudflareClient(token);
  await client.get<{ status?: string }>("/user/tokens/verify");
  await client.envelope<CloudflareZone[]>("/zones?per_page=1&page=1");
}

export async function upsertCloudflareARecord(
  token: string,
  host: string,
  address: string,
): Promise<void> {
  const client = new CloudflareClient(token);
  const zone = await resolveZone(client, host);
  const records = await listARecords(client, zone.id, host);
  const body = {
    type: "A",
    name: host,
    content: address,
    ttl: 1,
    proxied: false,
  };

  const existing = records[0];
  if (existing) {
    await client.put<CloudflareDnsRecord>(
      `/zones/${encodeURIComponent(zone.id)}/dns_records/${encodeURIComponent(existing.id)}`,
      body,
    );
    await Promise.all(
      records
        .slice(1)
        .map((record) =>
          client.delete(
            `/zones/${encodeURIComponent(zone.id)}/dns_records/${encodeURIComponent(record.id)}`,
          ),
        ),
    );
    return;
  }

  await client.post<CloudflareDnsRecord>(
    `/zones/${encodeURIComponent(zone.id)}/dns_records`,
    body,
  );
}

export async function deleteCloudflareARecord(token: string, host: string): Promise<void> {
  const client = new CloudflareClient(token);
  const zone = await resolveZone(client, host);
  const records = await listARecords(client, zone.id, host);
  await Promise.all(
    records.map((record) =>
      client.delete(
        `/zones/${encodeURIComponent(zone.id)}/dns_records/${encodeURIComponent(record.id)}`,
      ),
    ),
  );
}

export async function deleteCloudflareLoadBalancer(token: string, providerResourceId: string) {
  const ref = parseResourceRef(providerResourceId);
  const client = new CloudflareClient(token);

  await client.delete(
    `/zones/${encodeURIComponent(ref.zoneId)}/load_balancers/${encodeURIComponent(ref.loadBalancerId)}`,
  );
  await client.delete(
    `/accounts/${encodeURIComponent(ref.accountId)}/load_balancers/pools/${encodeURIComponent(ref.poolId)}`,
  );
  await client.delete(
    `/accounts/${encodeURIComponent(ref.accountId)}/load_balancers/monitors/${encodeURIComponent(ref.monitorId)}`,
  );
}

export async function syncCloudflareLoadBalancer(
  input: CloudflareSyncInput,
): Promise<CloudflareSyncResult> {
  if (input.targets.length === 0) {
    throw new Error("Attach at least one server before syncing the load balancer");
  }

  const client = new CloudflareClient(input.token);
  const zone = await resolveZone(client, input.host);
  const accountId = zone.account?.id;
  if (!accountId) {
    throw new Error(`Cloudflare zone ${zone.name} does not include an account id`);
  }

  const existingRef = input.providerResourceId ? parseResourceRef(input.providerResourceId) : null;
  const monitor = await upsertMonitor(client, input, zone, accountId, existingRef?.monitorId);
  const pool = await upsertPool(client, input, accountId, monitor.id, existingRef?.poolId);
  const loadBalancer = await upsertLoadBalancer(
    client,
    input,
    zone.id,
    pool.id,
    existingRef?.loadBalancerId,
  );

  const ref: CloudflareResourceRef = {
    zoneId: zone.id,
    accountId,
    loadBalancerId: loadBalancer.id,
    poolId: pool.id,
    monitorId: monitor.id,
  };

  return {
    providerResourceId: JSON.stringify(ref),
    endpointIpv4: null,
    endpointIpv6: null,
    targets: input.targets.map((target) => ({
      serverId: target.serverId,
      address: target.address,
      providerTargetId: originName(target),
    })),
  };
}

async function resolveZone(client: CloudflareClient, host: string): Promise<CloudflareZone> {
  const zones = await listZones(client);
  const normalizedHost = host.toLowerCase();
  const zone = zones
    .filter(
      (candidate) =>
        normalizedHost === candidate.name.toLowerCase() ||
        normalizedHost.endsWith(`.${candidate.name.toLowerCase()}`),
    )
    .sort((a, b) => b.name.length - a.name.length)[0];

  if (!zone) {
    throw new Error(`No Cloudflare zone found for ${host}`);
  }

  return zone;
}

async function listZones(client: CloudflareClient): Promise<CloudflareZone[]> {
  const zones: CloudflareZone[] = [];
  let page = 1;

  while (page) {
    const parsed = await client.envelope<CloudflareZone[]>(`/zones?per_page=50&page=${page}`);
    zones.push(...parsed.result);
    const totalPages = parsed?.result_info?.total_pages ?? 0;
    page = totalPages > page ? page + 1 : 0;
  }

  return zones;
}

async function listARecords(
  client: CloudflareClient,
  zoneId: string,
  host: string,
): Promise<CloudflareDnsRecord[]> {
  const params = new URLSearchParams({
    type: "A",
    name: host,
    per_page: "100",
  });
  return client.get<CloudflareDnsRecord[]>(
    `/zones/${encodeURIComponent(zoneId)}/dns_records?${params}`,
  );
}

async function upsertMonitor(
  client: CloudflareClient,
  input: CloudflareSyncInput,
  zone: CloudflareZone,
  accountId: string,
  monitorId: string | undefined,
): Promise<CloudflareMonitor> {
  const body = {
    type: "http",
    method: "GET",
    path: input.healthCheckPath,
    port: 80,
    expected_codes: "2xx",
    follow_redirects: true,
    allow_insecure: false,
    interval: 60,
    timeout: 5,
    retries: 2,
    consecutive_down: 2,
    consecutive_up: 2,
    probe_zone: zone.name,
    header: {
      Host: [input.host],
    },
    description: `Basse monitor for ${input.host}`,
  };

  if (monitorId) {
    return client.put<CloudflareMonitor>(
      `/accounts/${encodeURIComponent(accountId)}/load_balancers/monitors/${encodeURIComponent(monitorId)}`,
      body,
    );
  }

  return client.post<CloudflareMonitor>(
    `/accounts/${encodeURIComponent(accountId)}/load_balancers/monitors`,
    body,
  );
}

async function upsertPool(
  client: CloudflareClient,
  input: CloudflareSyncInput,
  accountId: string,
  monitorId: string,
  poolId: string | undefined,
): Promise<CloudflarePool> {
  const body = {
    name: poolName(input),
    description: `Basse origins for ${input.host}`,
    enabled: true,
    monitor: monitorId,
    minimum_origins: 1,
    origins: input.targets.map((target) => ({
      name: originName(target),
      address: normalizeAddress(target.address),
      enabled: true,
      weight: 1,
      header: {
        Host: [input.host],
      },
    })),
    origin_steering: {
      policy: "random",
    },
  };

  if (poolId) {
    return client.put<CloudflarePool>(
      `/accounts/${encodeURIComponent(accountId)}/load_balancers/pools/${encodeURIComponent(poolId)}`,
      body,
    );
  }

  return client.post<CloudflarePool>(
    `/accounts/${encodeURIComponent(accountId)}/load_balancers/pools`,
    body,
  );
}

async function upsertLoadBalancer(
  client: CloudflareClient,
  input: CloudflareSyncInput,
  zoneId: string,
  poolId: string,
  loadBalancerId: string | undefined,
): Promise<CloudflareLoadBalancer> {
  const body = {
    name: input.host,
    description: `Basse managed load balancer for ${input.name}`,
    enabled: true,
    proxied: true,
    ttl: 30,
    steering_policy: "off",
    default_pools: [poolId],
    fallback_pool: poolId,
  };

  if (loadBalancerId) {
    return client.put<CloudflareLoadBalancer>(
      `/zones/${encodeURIComponent(zoneId)}/load_balancers/${encodeURIComponent(loadBalancerId)}`,
      body,
    );
  }

  return client.post<CloudflareLoadBalancer>(
    `/zones/${encodeURIComponent(zoneId)}/load_balancers`,
    body,
  );
}

function parseResourceRef(value: string): CloudflareResourceRef {
  try {
    const parsed = JSON.parse(value) as Partial<CloudflareResourceRef>;
    if (
      parsed.zoneId &&
      parsed.accountId &&
      parsed.loadBalancerId &&
      parsed.poolId &&
      parsed.monitorId
    ) {
      return {
        zoneId: parsed.zoneId,
        accountId: parsed.accountId,
        loadBalancerId: parsed.loadBalancerId,
        poolId: parsed.poolId,
        monitorId: parsed.monitorId,
      };
    }
  } catch {
    // Existing rows should only contain JSON refs for Cloudflare resources.
  }

  throw new Error("Invalid Cloudflare load balancer reference");
}

function originName(target: SyncTarget): string {
  return sanitizeName(`${target.name}-${target.serverId.slice(0, 8)}`);
}

function poolName(input: CloudflareSyncInput): string {
  return sanitizeName(`basse-${input.appId.slice(0, 8)}-${input.host}`);
}

function sanitizeName(value: string): string {
  return value
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9_-]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 63);
}

function normalizeAddress(address: string): string {
  return address.trim().replace(/^\[|\]$/g, "");
}
