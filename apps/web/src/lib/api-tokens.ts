import type { ApiToken, CreateApiTokenInput, CreateApiTokenResult } from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listApiTokens(): Promise<ApiToken[]> {
  const response = await fetch(`${apiBaseUrl}/api/api-tokens`, { credentials: "include" });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ApiToken[]>;
}

export async function createApiToken(input: CreateApiTokenInput): Promise<CreateApiTokenResult> {
  const response = await fetch(`${apiBaseUrl}/api/api-tokens`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<CreateApiTokenResult>;
}

export async function deleteApiToken(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/api-tokens/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
