import type { UpdateWorkspaceSettingsInput, WorkspaceSettings } from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getWorkspaceSettings(): Promise<WorkspaceSettings> {
  const response = await fetch(`${apiBaseUrl}/api/workspace/settings`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<WorkspaceSettings>;
}

export async function updateWorkspaceSettings(
  input: UpdateWorkspaceSettingsInput,
): Promise<WorkspaceSettings> {
  const response = await fetch(`${apiBaseUrl}/api/workspace/settings`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<WorkspaceSettings>;
}
