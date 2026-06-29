import type { CreateSshKeyInput, SshKey } from "@basse/shared";

export type { SshKey };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listSshKeys(): Promise<SshKey[]> {
  const response = await fetch(`${apiBaseUrl}/api/ssh-keys`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<SshKey[]>;
}

export async function createSshKey(input: CreateSshKeyInput): Promise<SshKey> {
  const response = await fetch(`${apiBaseUrl}/api/ssh-keys`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<SshKey>;
}

export async function deleteSshKey(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/ssh-keys/${id}`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
