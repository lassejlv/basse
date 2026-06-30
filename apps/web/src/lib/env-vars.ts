import type {
  EnvReferenceSuggestion,
  EnvVarMasked,
  EnvVarPlain,
  SharedEnvVarMasked,
  SharedEnvVarPlain,
} from "@basse/shared";

export type {
  EnvReferenceSuggestion,
  EnvVarMasked,
  EnvVarPlain,
  SharedEnvVarMasked,
  SharedEnvVarPlain,
};

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

export async function listEnvReferences(appId: string): Promise<EnvReferenceSuggestion[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/env-references`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<EnvReferenceSuggestion[]>;
}

export async function listProjectSharedEnvVars(projectId: string): Promise<SharedEnvVarMasked[]> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/shared-env-vars`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<SharedEnvVarMasked[]>;
}

export async function revealProjectSharedEnvVars(projectId: string): Promise<SharedEnvVarPlain[]> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/shared-env-vars/reveal`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<SharedEnvVarPlain[]>;
}

export async function setProjectSharedEnvVars(
  projectId: string,
  vars: { key: string; value: string }[],
): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${projectId}/shared-env-vars`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ vars }),
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function listEnvironmentSharedEnvVars(
  environmentId: string,
): Promise<SharedEnvVarMasked[]> {
  const response = await fetch(`${apiBaseUrl}/api/environments/${environmentId}/shared-env-vars`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<SharedEnvVarMasked[]>;
}

export async function revealEnvironmentSharedEnvVars(
  environmentId: string,
): Promise<SharedEnvVarPlain[]> {
  const response = await fetch(
    `${apiBaseUrl}/api/environments/${environmentId}/shared-env-vars/reveal`,
    { credentials: "include" },
  );
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<SharedEnvVarPlain[]>;
}

export async function setEnvironmentSharedEnvVars(
  environmentId: string,
  vars: { key: string; value: string }[],
): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/environments/${environmentId}/shared-env-vars`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ vars }),
  });
  if (!response.ok) throw new Error(await parseError(response));
}
