import type { Alert, AlertsOverview } from "@basse/shared";

export type { Alert, AlertsOverview };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listAlerts(status: "active" | "resolved" | "all" = "active"): Promise<Alert[]> {
  const response = await fetch(`${apiBaseUrl}/api/alerts?status=${encodeURIComponent(status)}`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<Alert[]>;
}

export async function getAlertsOverview(): Promise<AlertsOverview> {
  const response = await fetch(`${apiBaseUrl}/api/alerts/overview`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AlertsOverview>;
}

export async function acknowledgeAlert(alertId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/alerts/${alertId}/acknowledge`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function resolveAlert(alertId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/alerts/${alertId}/resolve`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
