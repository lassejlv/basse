const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export type SystemInfo = {
  mode: "cloud" | "self-hosted";
  selfHosted: boolean;
  currentCommitSha: string;
  installUrl: string;
  updateUrl: string;
  updateCommand: string;
};

export type UpdateCheck = {
  mode: "cloud" | "self-hosted";
  selfHosted: boolean;
  currentCommitSha: string;
  latestCommitSha: string | null;
  updateAvailable: boolean | null;
  updateCommand: string | null;
  message: string;
};

export async function getSystemInfo(): Promise<SystemInfo> {
  const response = await fetch(`${apiBaseUrl}/api/system`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<SystemInfo>;
}

export async function checkSystemUpdate(): Promise<UpdateCheck> {
  const response = await fetch(`${apiBaseUrl}/api/system/update-check`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<UpdateCheck>;
}
