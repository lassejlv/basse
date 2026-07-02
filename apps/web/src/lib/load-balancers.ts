import type {
  CreateLoadBalancerIntegrationInput,
  CreateManagedLoadBalancerInput,
  LoadBalancerIntegration,
  LoadBalancerEvent,
  ManagedLoadBalancer,
} from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listLoadBalancerIntegrations(): Promise<LoadBalancerIntegration[]> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/integrations`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<LoadBalancerIntegration[]>;
}

export async function saveLoadBalancerIntegration(
  input: CreateLoadBalancerIntegrationInput,
): Promise<LoadBalancerIntegration> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/integrations`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<LoadBalancerIntegration>;
}

export async function deleteLoadBalancerIntegration(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/integrations/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function listManagedLoadBalancers(appId: string): Promise<ManagedLoadBalancer[]> {
  const response = await fetch(
    `${apiBaseUrl}/api/load-balancers?appId=${encodeURIComponent(appId)}`,
    { credentials: "include" },
  );
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ManagedLoadBalancer[]>;
}

export async function createManagedLoadBalancer(
  input: CreateManagedLoadBalancerInput,
): Promise<ManagedLoadBalancer> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ManagedLoadBalancer>;
}

export async function syncManagedLoadBalancer(id: string): Promise<ManagedLoadBalancer> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/${id}/sync`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<ManagedLoadBalancer>;
}

export async function listLoadBalancerEvents(id: string): Promise<LoadBalancerEvent[]> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/${id}/events`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<LoadBalancerEvent[]>;
}

export async function deleteManagedLoadBalancer(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/load-balancers/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
