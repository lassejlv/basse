import type {
  DatabaseBackup,
  DatabaseBackupList,
  DatabaseBackupSettings,
  UpdateDatabaseBackupSettingsInput,
} from "@basse/shared";

export type { DatabaseBackup, DatabaseBackupList, DatabaseBackupSettings };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listBackups(appId: string): Promise<DatabaseBackupList> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<DatabaseBackupList>;
}

export async function createBackup(appId: string): Promise<DatabaseBackup> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<DatabaseBackup>;
}

export async function restoreBackup(appId: string, backupId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups/${backupId}/restore`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function deleteBackup(appId: string, backupId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups/${backupId}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export function backupDownloadUrl(appId: string, backupId: string): string {
  return `${apiBaseUrl}/api/apps/${appId}/backups/${backupId}/download`;
}

export async function updateBackupSettings(
  appId: string,
  input: UpdateDatabaseBackupSettingsInput,
): Promise<DatabaseBackupSettings> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups/settings`, {
    method: "PATCH",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<DatabaseBackupSettings>;
}
