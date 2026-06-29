import type { DepotConnection, SaveDepotConnectionInput } from "@basse/shared";

export type { DepotConnection };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getDepotConnection(): Promise<DepotConnection> {
  const response = await fetch(`${apiBaseUrl}/api/depot`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DepotConnection>;
}

export async function saveDepotConnection(
  input: SaveDepotConnectionInput,
): Promise<DepotConnection> {
  const response = await fetch(`${apiBaseUrl}/api/depot`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<DepotConnection>;
}

export async function disconnectDepot(): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/depot`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
