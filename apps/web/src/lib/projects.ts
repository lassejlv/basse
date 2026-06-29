export type Project = {
  id: string;
  organizationId: string;
  name: string;
  slug: string;
  createdAt: string;
  updatedAt: string;
};

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
