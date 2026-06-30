import type { CreateDomainInput, Domain } from "@basse/shared";

export type { Domain };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listDomains(serverId: string): Promise<Domain[]> {
  const response = await fetch(`${apiBaseUrl}/api/domains?serverId=${encodeURIComponent(serverId)}`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Domain[]>;
}

export async function createDomain(
  serverId: string,
  input: CreateDomainInput,
): Promise<Domain> {
  const response = await fetch(`${apiBaseUrl}/api/domains`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ serverId, ...input }),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Domain>;
}

export async function deleteDomain(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/domains/${id}`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function resyncProxy(serverId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${serverId}/proxy/resync`, {
    method: "POST",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
