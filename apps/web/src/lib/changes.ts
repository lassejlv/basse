import type {
  AppStagedChanges,
  ApplyStagedChangesResult,
  EnvVarPlain,
  ProjectApplyStagedChangesResult,
  ProjectStagedChangeHistoryEntry,
  ProjectStagedChanges,
  StageAppChangesInput,
  StageDomainChangeInput,
  StageEnvVarsInput,
  StagedChange,
  StagedChangeHistoryEntry,
} from "@basse/shared";

export type {
  AppStagedChanges,
  ApplyStagedChangesResult,
  ProjectApplyStagedChangesResult,
  ProjectStagedChangeHistoryEntry,
  ProjectStagedChanges,
  StagedChange,
  StagedChangeHistoryEntry,
};

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

export async function getChangeHistory(appId: string): Promise<StagedChangeHistoryEntry[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/history`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<StagedChangeHistoryEntry[]>;
}

export async function getProjectChanges(projectId: string): Promise<ProjectStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/changes`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ProjectStagedChanges>;
}

export async function getProjectChangeHistory(
  projectId: string,
): Promise<ProjectStagedChangeHistoryEntry[]> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/changes/history`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ProjectStagedChangeHistoryEntry[]>;
}

export async function applyProjectChanges(
  projectId: string,
): Promise<ProjectApplyStagedChangesResult> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/changes/apply`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ProjectApplyStagedChangesResult>;
}

export async function discardProjectChanges(projectId: string): Promise<ProjectStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/changes/discard`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ProjectStagedChanges>;
}

export async function discardProjectChange(
  projectId: string,
  changeId: string,
): Promise<ProjectStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/changes/${changeId}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ProjectStagedChanges>;
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

export async function stageDomainChange(
  appId: string,
  input: StageDomainChangeInput,
): Promise<AppStagedChanges> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/changes/domain`, {
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
