import type {
  AgentInfo,
  AgentLogs,
  AgentMetrics,
  AgentUpdateCheck,
  CreateServerInput,
  Server,
} from "@basse/shared";

export type { Server };

const apiBaseUrl = import.meta.env.VITE_API_URL ?? "";

async function parseError(response: Response): Promise<string> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null;
  return body?.error ?? `Request failed with ${response.status}`;
}

export async function listServers(): Promise<Server[]> {
  const response = await fetch(`${apiBaseUrl}/api/servers`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server[]>;
}

export async function getServer(id: string): Promise<Server> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}`, {
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server>;
}

export async function createServer(input: CreateServerInput): Promise<Server> {
  const response = await fetch(`${apiBaseUrl}/api/servers`, {
    method: "POST",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(input),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<Server>;
}

export type ConnectionCheck = {
  ok: boolean;
  fingerprint: string | null;
  error?: string;
};

export async function checkServerConnection(id: string): Promise<ConnectionCheck> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/check-connection`, {
    method: "POST",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }

  return response.json() as Promise<ConnectionCheck>;
}

export async function provisionServer(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/provision`, {
    method: "POST",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function getAgentInfo(id: string): Promise<AgentInfo> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/agent`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AgentInfo>;
}

export async function getAgentMetrics(id: string): Promise<AgentMetrics> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/agent/metrics`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AgentMetrics>;
}

export async function getAgentLogs(id: string): Promise<AgentLogs> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/agent/logs?tail=250`, {
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AgentLogs>;
}

export async function checkAgentUpdate(id: string): Promise<AgentUpdateCheck> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/agent/check-update`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
  return response.json() as Promise<AgentUpdateCheck>;
}

export async function updateAgent(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/agent/update`, {
    method: "POST",
    credentials: "include",
  });
  if (!response.ok) throw new Error(await parseError(response));
}

export async function sendServerDeleteCode(id: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}/delete-code`, {
    method: "POST",
    credentials: "include",
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}

export async function deleteServer(id: string, code: string): Promise<void> {
  const response = await fetch(`${apiBaseUrl}/api/servers/${id}`, {
    method: "DELETE",
    credentials: "include",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ code }),
  });

  if (!response.ok) {
    throw new Error(await parseError(response));
  }
}
