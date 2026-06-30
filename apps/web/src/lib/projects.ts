import type { Project, ProjectListItem } from "@basse/shared";

export type { Project, ProjectListItem };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listProjects(): Promise<ProjectListItem[]> {
  const response = await fetch(`${apiBaseUrl}/api/projects`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<ProjectListItem[]>;
}

export async function getProject(id: string): Promise<Project> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${id}`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Project>;
}

export async function createProject(name: string): Promise<Project> {
  const response = await fetch(`${apiBaseUrl}/api/projects`, {
    method: "POST",
    credentials: "include",
    headers: {
      "content-type": "application/json",
    },
    body: JSON.stringify({ name }),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Project>;
}

export async function deleteProject(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${id}`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
