import { db, digitaloceanConnection } from "@basse/db";
import type { DigitalOceanRegion, DigitalOceanSize } from "@basse/shared";
import { eq } from "drizzle-orm";
import { decryptSecret } from "../lib/crypto";

const DO_API = "https://api.digitalocean.com/v2";

type DigitalOceanErrorBody = {
  id?: string;
  message?: string;
};

type DoRegion = {
  slug: string;
  name: string;
  available: boolean;
  sizes: string[];
};

type DoSize = {
  slug: string;
  description: string;
  available: boolean;
  vcpus: number;
  memory: number;
  disk: number;
  price_monthly: number;
  regions: string[];
};

type DoDroplet = {
  id: number;
  name: string;
  status: "new" | "active" | "off" | "archive";
  networks?: {
    v4?: { ip_address: string; type: "public" | "private" }[];
  };
};

type DoSshKey = {
  id: number;
  fingerprint: string;
  public_key: string;
};

async function request<T>(token: string, method: string, path: string, body?: unknown): Promise<T> {
  const response = await fetch(`${DO_API}${path}`, {
    method,
    headers: {
      authorization: `Bearer ${token}`,
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });

  if (!response.ok) {
    const parsed = (await response.json().catch(() => null)) as DigitalOceanErrorBody | null;
    const message = parsed?.message ?? `request failed with ${response.status}`;
    throw new Error(`DigitalOcean: ${message}`);
  }

  if (response.status === 204) {
    return undefined as T;
  }

  return response.json() as Promise<T>;
}

export async function validateDigitalOceanToken(token: string): Promise<void> {
  await request<{ account: unknown }>(token, "GET", "/account");
}

/** Returns the workspace's decrypted DigitalOcean API token, or null. */
export async function getDigitalOceanToken(organizationId: string): Promise<string | null> {
  const [connection] = await db
    .select()
    .from(digitaloceanConnection)
    .where(eq(digitaloceanConnection.organizationId, organizationId))
    .limit(1);

  if (!connection) return null;
  return decryptSecret(connection.apiToken);
}

export async function listDigitalOceanRegions(token: string): Promise<DigitalOceanRegion[]> {
  const response = await request<{ regions: DoRegion[] }>(token, "GET", "/regions?per_page=200");
  return response.regions
    .filter((region) => region.available)
    .map((region) => ({
      slug: region.slug,
      name: region.name,
      sizes: region.sizes,
    }));
}

export async function listDigitalOceanSizes(token: string): Promise<DigitalOceanSize[]> {
  const response = await request<{ sizes: DoSize[] }>(token, "GET", "/sizes?per_page=200");
  return response.sizes
    .filter((size) => size.available)
    .map((size) => ({
      slug: size.slug,
      description: size.description,
      vcpus: size.vcpus,
      memory: size.memory,
      disk: size.disk,
      priceMonthly: size.price_monthly,
      regions: size.regions,
    }));
}

/**
 * Registers the public key on the DO account (idempotent on fingerprint) and
 * returns its DO key id. Droplet creation references keys by id.
 */
async function ensureSshKey(token: string, name: string, publicKey: string): Promise<number> {
  try {
    const created = await request<{ ssh_key: DoSshKey }>(token, "POST", "/account/keys", {
      name,
      public_key: publicKey,
    });
    return created.ssh_key.id;
  } catch (error) {
    // 422 "SSH Key is already in use on your account" — look up its id instead.
    if (error instanceof Error && /already in use/i.test(error.message)) {
      const existing = await request<{ ssh_keys: DoSshKey[] }>(
        token,
        "GET",
        "/account/keys?per_page=200",
      );
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

export type CreateDropletInput = {
  token: string;
  name: string;
  region: string;
  size: string;
  sshPublicKey: string;
  serverId: string;
};

/** Creates a droplet and returns its DO id. The droplet boots asynchronously. */
export async function createDigitalOceanDroplet(input: CreateDropletInput): Promise<string> {
  const keyId = await ensureSshKey(input.token, `basse-${input.serverId}`, input.sshPublicKey);

  const created = await request<{ droplet: DoDroplet }>(input.token, "POST", "/droplets", {
    name: input.name,
    region: input.region,
    size: input.size,
    image: "ubuntu-24-04-x64",
    ssh_keys: [keyId],
    tags: ["basse-managed", `basse-server-${input.serverId}`],
  });

  return String(created.droplet.id);
}

export type DropletState = {
  status: DoDroplet["status"];
  publicIpv4: string | null;
};

export async function getDigitalOceanDroplet(
  token: string,
  dropletId: string,
): Promise<DropletState> {
  const response = await request<{ droplet: DoDroplet }>(
    token,
    "GET",
    `/droplets/${encodeURIComponent(dropletId)}`,
  );
  const publicIpv4 =
    response.droplet.networks?.v4?.find((network) => network.type === "public")?.ip_address ?? null;
  return { status: response.droplet.status, publicIpv4 };
}

export async function deleteDigitalOceanDroplet(token: string, dropletId: string): Promise<void> {
  await request<void>(token, "DELETE", `/droplets/${encodeURIComponent(dropletId)}`);
}
