import type {
  CreateNeonBranchInput,
  NeonBranch,
  NeonBranchConnection,
  NeonConnection,
  NeonRegion,
  SaveNeonConnectionInput,
} from "@basse/shared";

export type { NeonBranch, NeonBranchConnection, NeonConnection, NeonRegion };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function getNeonConnection(): Promise<NeonConnection> {
  const response = await fetch(`${apiBaseUrl}/api/neon`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonConnection>;
}

export async function saveNeonConnection(input: SaveNeonConnectionInput): Promise<NeonConnection> {
  const response = await fetch(`${apiBaseUrl}/api/neon`, {
    method: "PUT",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonConnection>;
}

export async function disconnectNeon(): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/neon`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function listNeonRegions(): Promise<NeonRegion[]> {
  const response = await fetch(`${apiBaseUrl}/api/neon/regions`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonRegion[]>;
}

export async function listNeonBranches(appId: string): Promise<NeonBranch[]> {
  const response = await fetch(`${apiBaseUrl}/api/neon/apps/${appId}/branches`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonBranch[]>;
}

export async function createNeonBranch(
  appId: string,
  input: CreateNeonBranchInput,
): Promise<NeonBranch> {
  const response = await fetch(`${apiBaseUrl}/api/neon/apps/${appId}/branches`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonBranch>;
}

export async function deleteNeonBranch(appId: string, branchId: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/neon/apps/${appId}/branches/${branchId}`, {
    method: "DELETE",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function getNeonBranchConnection(
  appId: string,
  branchId: string,
): Promise<NeonBranchConnection> {
  const response = await fetch(
    `${apiBaseUrl}/api/neon/apps/${appId}/branches/${branchId}/connection`,
    { credentials: "include" },
  );

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<NeonBranchConnection>;
}
