import type {
  AppStagedChanges,
  ApplyStagedChangesResult,
  EnvVarPlain,
  StageAppChangesInput,
  StageEnvVarsInput,
  StagedChange,
} from "@basse/shared";

export type { AppStagedChanges, ApplyStagedChangesResult, StagedChange };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getChanges(appId: string): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppStagedChanges>;
}

export async function stageAppChanges(
  appId: string,
  input: StageAppChangesInput,
): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/app`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppStagedChanges>;
}

export async function stageEnvVars(
  appId: string,
  input: StageEnvVarsInput,
): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/env`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppStagedChanges>;
}

// Draft env (current ⊕ staged) plaintext, for seeding the env editor.
export async function getEnvDraft(appId: string): Promise<EnvVarPlain[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/env-draft`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<EnvVarPlain[]>;
}

export async function applyChanges(appId: string): Promise<ApplyStagedChangesResult> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/apply`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ApplyStagedChangesResult>;
}

export async function discardChanges(appId: string): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/discard`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppStagedChanges>;
}

export async function discardChange(appId: string, changeId: string): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/${changeId}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AppStagedChanges>;
}
