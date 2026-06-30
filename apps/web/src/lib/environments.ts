import type { Environment } from "@basse/shared";

export type { Environment };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listEnvironments(projectId: string): Promise<Environment[]> {
  const response = await fetch(
    `${apiBaseUrl}/api/environments?projectId=${encodeURIComponent(projectId)}`,
    { credentials: "include" },
  );
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Environment[]>;
}

export async function createEnvironment(projectId: string, name: string): Promise<Environment> {
  const response = await fetch(`${apiBaseUrl}/api/environments`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ projectId, name }),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Environment>;
}
