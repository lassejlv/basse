import type { CreateCronJobInput, CronJob, UpdateCronJobInput } from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listCronJobs(appId: string): Promise<CronJob[]> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/cron-jobs`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<CronJob[]>;
}

export async function createCronJob(appId: string, input: CreateCronJobInput): Promise<CronJob> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/cron-jobs`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<CronJob>;
}

export async function updateCronJob(
  appId: string,
  jobId: string,
  input: UpdateCronJobInput,
): Promise<CronJob> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/cron-jobs/${jobId}`, {
    method: "PATCH",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<CronJob>;
}

export async function deleteCronJob(appId: string, jobId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/cron-jobs/${jobId}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function runCronJob(appId: string, jobId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/apps/${appId}/cron-jobs/${jobId}/run`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}
