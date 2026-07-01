import type { Deployment, TriggerDeploymentInput } from "@basse/shared";

export type { Deployment };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listDeployments(appId: string): Promise<Deployment[]> {
  const response = await fetch(`${apiBaseUrl}/api/deployments?appId=${encodeURIComponent(appId)}`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Deployment[]>;
}

export async function getDeployment(id: string): Promise<Deployment> {
  const response = await fetch(`${apiBaseUrl}/api/deployments/${id}`, { credentials: "include" });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Deployment>;
}

export async function triggerDeploy(
  appId: string,
  options: Omit<TriggerDeploymentInput, "appId"> = {},
): Promise<Deployment> {
  const response = await fetch(`${apiBaseUrl}/api/deployments`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ appId, ...options }),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Deployment>;
}

export async function rollbackDeployment(deploymentId: string): Promise<Deployment> {
  const response = await fetch(`${apiBaseUrl}/api/deployments/rollback`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ deploymentId }),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Deployment>;
}
