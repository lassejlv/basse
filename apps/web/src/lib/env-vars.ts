import type { EnvVarMasked, EnvVarPlain } from "@basse/shared";

export type { EnvVarMasked, EnvVarPlain };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listEnvVars(appId: string): Promise<EnvVarMasked[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/env-vars`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<EnvVarMasked[]>;
}

export async function revealEnvVars(appId: string): Promise<EnvVarPlain[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/env-vars/reveal`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<EnvVarPlain[]>;
}

export async function setEnvVars(
  appId: string,
  vars: { key: string; value: string }[],
): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/env-vars`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ vars }),
  });
  if (!response.ok) throw new Error(await parseError(response));
}
