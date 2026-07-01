import type {
  CompleteGitHubAppManifestInput,
  GitHubAppInstallation,
  GitHubAppIntegration,
  GitHubAppManifest,
  GitHubRepositoryList,
  SaveGitHubAppInstallationInput,
} from "@basse/shared";

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getGitHubAppIntegration(): Promise<GitHubAppIntegration> {
  const response = await fetch(`${apiBaseUrl}/api/github/integration`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubAppIntegration>;
}

export async function disconnectGitHubApp(): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/github/integration`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function getGitHubAppManifest(): Promise<GitHubAppManifest> {
  const origin = encodeURIComponent(window.location.origin);
  const response = await fetch(`${apiBaseUrl}/api/github/manifest?origin=${origin}`, {
    cache: "no-store",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubAppManifest>;
}

export async function completeGitHubAppManifest(
  input: CompleteGitHubAppManifestInput,
): Promise<GitHubAppIntegration> {
  const response = await fetch(`${apiBaseUrl}/api/github/manifest/complete`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubAppIntegration>;
}

export async function listGitHubAppInstallations(): Promise<GitHubAppInstallation[]> {
  const response = await fetch(`${apiBaseUrl}/api/github/installations`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubAppInstallation[]>;
}

export async function saveGitHubAppInstallation(
  input: SaveGitHubAppInstallationInput,
): Promise<GitHubAppInstallation> {
  const response = await fetch(`${apiBaseUrl}/api/github/installations`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubAppInstallation>;
}

export async function deleteGitHubAppInstallation(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/github/installations/${id}`, {
    method: "DELETE",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function listGitHubRepositories(): Promise<GitHubRepositoryList> {
  const response = await fetch(`${apiBaseUrl}/api/github/repositories`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<GitHubRepositoryList>;
}
