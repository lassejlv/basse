import type {
  CreateDigitalOceanServerInput,
  DigitalOceanConnection,
  DigitalOceanRegion,
  DigitalOceanSize,
  SaveDigitalOceanConnectionInput,
} from "@basse/shared";

export type { DigitalOceanConnection, DigitalOceanRegion, DigitalOceanSize };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getDigitalOceanConnection(): Promise<DigitalOceanConnection> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DigitalOceanConnection>;
}

export async function saveDigitalOceanConnection(
  input: SaveDigitalOceanConnectionInput,
): Promise<DigitalOceanConnection> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DigitalOceanConnection>;
}

export async function disconnectDigitalOcean(): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function listDigitalOceanRegions(): Promise<DigitalOceanRegion[]> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean/regions`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DigitalOceanRegion[]>;
}

export async function listDigitalOceanSizes(): Promise<DigitalOceanSize[]> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean/sizes`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DigitalOceanSize[]>;
}

export async function createDigitalOceanServer(
  input: CreateDigitalOceanServerInput,
): Promise<{ id: string }> {
  const response = await fetch(`${apiBaseUrl}/api/digitalocean/servers`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<{ id: string }>;
}
