import type {
  CreateHetznerServerInput,
  HetznerConnection,
  HetznerLocation,
  HetznerServerType,
  SaveHetznerConnectionInput,
} from "@basse/shared";

export type { HetznerConnection, HetznerLocation, HetznerServerType };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getHetznerConnection(): Promise<HetznerConnection> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<HetznerConnection>;
}

export async function saveHetznerConnection(
  input: SaveHetznerConnectionInput,
): Promise<HetznerConnection> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<HetznerConnection>;
}

export async function disconnectHetzner(): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function listHetznerLocations(): Promise<HetznerLocation[]> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner/locations`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<HetznerLocation[]>;
}

export async function listHetznerServerTypes(): Promise<HetznerServerType[]> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner/server-types`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<HetznerServerType[]>;
}

export async function createHetznerServer(
  input: CreateHetznerServerInput,
): Promise<{ id: string }> {
  const response = await fetch(`${apiBaseUrl}/api/hetzner/servers`, {
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
