import type { Project } from "@basse/shared";

export type { Project };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

export async function listProjects(): Promise<Project[]> {
  const response = await fetch(`${apiBaseUrl}/api/projects`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(`Projects request failed with ${response.status}`);
  }

  return response.json() as Promise<Project[]>;
}

export async function getProject(id: string): Promise<Project> {
  const response = await fetch(`${apiBaseUrl}/api/projects/${id}`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(`Project request failed with ${response.status}`);
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
    throw new Error(`Create project request failed with ${response.status}`);
  }

  return response.json() as Promise<Project>;
}
