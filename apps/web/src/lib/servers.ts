import type { CreateServerInput, Server } from "@basse/shared";

export type { Server };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listServers(): Promise<Server[]> {
  const response = await fetch(`${apiBaseUrl}/api/servers`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server[]>;
}

export async function getServer(id: string): Promise<Server> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server>;
}

export async function createServer(input: CreateServerInput): Promise<Server> {
  const response = await fetch(`${apiBaseUrl}/api/servers`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server>;
}

export async function deleteServer(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
