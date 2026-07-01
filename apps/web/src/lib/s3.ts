import type { CreateS3ConnectionInput, S3Connection, UpdateS3ConnectionInput } from "@basse/shared";

export type { S3Connection, CreateS3ConnectionInput, UpdateS3ConnectionInput };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listS3Connections(): Promise<S3Connection[]> {
  const response = await fetch(`${apiBaseUrl}/api/s3`, { credentials: "include" });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<S3Connection[]>;
}

export async function createS3Connection(input: CreateS3ConnectionInput): Promise<S3Connection> {
  const response = await fetch(`${apiBaseUrl}/api/s3`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<S3Connection>;
}

export async function updateS3Connection(
  id: string,
  input: UpdateS3ConnectionInput,
): Promise<S3Connection> {
  const response = await fetch(`${apiBaseUrl}/api/s3/${id}`, {
    method: "PATCH",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<S3Connection>;
}

export async function testS3Connection(id: string): Promise<S3Connection> {
  const response = await fetch(`${apiBaseUrl}/api/s3/${id}/test`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<S3Connection>;
}

export async function deleteS3Connection(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/s3/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function uploadBackupToS3(appId: string, backupId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/backups/${backupId}/upload`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
