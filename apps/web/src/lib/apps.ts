import type { App, CreateAppInput, UpdateAppInput } from "@basse/shared";

export type { App };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listApps(environmentId: string): Promise<App[]> {
  const response = await fetch(
    `${apiBaseUrl}/api/apps?environmentId=${encodeURIComponent(environmentId)}`,
    { credentials: "include" },
  );
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<App[]>;
}

export async function getApp(id: string): Promise<App> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}`, { credentials: "include" });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<App>;
}

export async function createApp(input: CreateAppInput): Promise<App> {
  const response = await fetch(`${apiBaseUrl}/api/apps`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<App>;
}

export async function updateApp(id: string, input: UpdateAppInput): Promise<App> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}`, {
    method: "PATCH",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<App>;
}

export async function deleteApp(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
