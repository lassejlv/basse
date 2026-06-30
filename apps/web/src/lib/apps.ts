import type {
  App,
  AppConsoleResult,
  DatabaseConnectionInfo,
  AppLogs,
  AppMetrics,
  CreateAppInput,
  ImportDockerContainerInput,
  ImportableDockerContainer,
  UpdateAppInput,
} from "@basse/shared";

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

export async function listImportableDockerContainers(
  serverId: string,
): Promise<ImportableDockerContainer[]> {
  const response = await fetch(
    `${apiBaseUrl}/api/apps/importable-containers?serverId=${encodeURIComponent(serverId)}`,
    { credentials: "include" },
  );
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ImportableDockerContainer[]>;
}

export async function importDockerContainer(input: ImportDockerContainerInput): Promise<App> {
  const response = await fetch(`${apiBaseUrl}/api/apps/import-container`, {
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

export async function getDatabaseConnectionInfo(id: string): Promise<DatabaseConnectionInfo> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}/database/connection`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<DatabaseConnectionInfo>;
}

export async function getAppMetrics(id: string, serverId?: string): Promise<AppMetrics> {
  const query = serverId ? `?serverId=${encodeURIComponent(serverId)}` : "";
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}/metrics${query}`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppMetrics>;
}

export async function getAppLogs(id: string, serverId?: string): Promise<AppLogs> {
  const params = new URLSearchParams({ tail: "250" });
  if (serverId) params.set("serverId", serverId);
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}/logs?${params}`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppLogs>;
}

export async function runAppConsoleCommand(
  id: string,
  input: { command: string; serverId?: string },
): Promise<AppConsoleResult> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}/console`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppConsoleResult>;
}

export async function stopAppContainer(
  id: string,
  input: { serverId?: string },
): Promise<{ ok: true }> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}/stop`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<{ ok: true }>;
}

export async function deleteApp(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
