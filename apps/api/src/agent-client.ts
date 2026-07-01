import type { DesiredDomain, ProxyStatus } from "@basse/shared";
import { sendOutboundAgentRequest } from "./outbound-agent";
import type { SshConnection } from "./ssh";
import { withTunnel } from "./ssh";

// Talks to the on-server Go agent. The agent listens on the server's loopback
// only, so every call goes through an SSH local-port-forward (withTunnel). The
// bearer token therefore never crosses the public network.

export const AGENT_PORT = 8888;

export type OutboundAgentConnection = {
  mode: "outbound";
  serverId: string;
};

export type AgentConnection = SshConnection | OutboundAgentConnection;

export type AgentVersion = {
  version: string;
};

export type AgentHealth = {
  reachable: boolean;
  ready: boolean;
  version?: string;
  error?: string;
};

export type AgentInfo = {
  agent: { version: string };
  docker: {
    Containers: number;
    ContainersRunning: number;
    Images: number;
    NCPU: number;
    MemTotal: number;
  };
  engine: {
    Version: string;
    ApiVersion: string;
    Os: string;
    Arch: string;
  };
};

function isOutboundConnection(conn: AgentConnection): conn is OutboundAgentConnection {
  return "mode" in conn && conn.mode === "outbound";
}

async function fetchAgent(
  conn: AgentConnection,
  token: string,
  input: {
    method: string;
    path: string;
    body?: unknown;
    authed?: boolean;
    timeoutMs: number;
  },
): Promise<{ status: number; ok: boolean; text: string }> {
  if (isOutboundConnection(conn)) {
    const result = await sendOutboundAgentRequest({
      serverId: conn.serverId,
      method: input.method,
      path: input.path,
      body: input.body,
      timeoutMs: input.timeoutMs,
    });
    return {
      status: result.status,
      ok: result.status >= 200 && result.status < 300,
      text: result.body,
    };
  }

  return withTunnel(conn, AGENT_PORT, async (baseUrl) => {
    const response = await fetch(`${baseUrl}${input.path}`, {
      method: input.method,
      headers: input.authed
        ? {
            authorization: `Bearer ${token}`,
            ...(typeof input.body === "undefined" ? {} : { "content-type": "application/json" }),
          }
        : typeof input.body === "undefined"
          ? undefined
          : { "content-type": "application/json" },
      body: typeof input.body === "undefined" ? undefined : JSON.stringify(input.body),
      signal: AbortSignal.timeout(input.timeoutMs),
    });
    return { status: response.status, ok: response.ok, text: await response.text() };
  });
}

function parseJson<T>(text: string): T {
  return (text ? JSON.parse(text) : null) as T;
}

async function getJson<T>(
  conn: AgentConnection,
  token: string,
  path: string,
  authed: boolean,
): Promise<T> {
  const response = await fetchAgent(conn, token, {
    method: "GET",
    path,
    authed,
    timeoutMs: 10_000,
  });

  if (!response.ok) {
    throw new Error(`agent ${path} returned ${response.status}`);
  }

  return parseJson<T>(response.text);
}

async function postJson<T>(
  conn: AgentConnection,
  token: string,
  path: string,
  body: unknown,
  timeoutMs: number,
): Promise<T> {
  const response = await fetchAgent(conn, token, {
    method: "POST",
    path,
    body,
    authed: true,
    timeoutMs,
  });

  if (!response.ok) {
    const detail = response.text
      ? (() => {
          try {
            return parseJson<{ error?: string }>(response.text);
          } catch {
            return null;
          }
        })()
      : null;
    throw new Error(detail?.error ?? `agent ${path} returned ${response.status}`);
  }

  return parseJson<T>(response.text);
}

/**
 * Brings up Caddy on the server (idempotent). Opens a tunnel and calls
 * /v1/proxy/ensure with a generous timeout (image pull + container start +
 * admin wait). Throws on failure.
 */
export async function ensureProxy(conn: AgentConnection, token: string): Promise<ProxyStatus> {
  return postJson<ProxyStatus>(conn, token, "/v1/proxy/ensure", {}, 170_000);
}

export async function getAgentInfo(conn: AgentConnection, token: string): Promise<AgentInfo> {
  return getJson<AgentInfo>(conn, token, "/v1/info", true);
}

/** Pushes the full desired domain set to the server's Caddy. Throws on failure. */
export async function syncDomains(
  conn: AgentConnection,
  token: string,
  domains: DesiredDomain[],
): Promise<void> {
  await postJson(conn, token, "/v1/proxy/sync", { domains }, 30_000);
}

export type DeployAppInput = {
  appId: string;
  image: string;
  cmd?: string[];
  port: number;
  env: Record<string, string>;
  registry?: { host: string; user: string; token: string };
  pullImage?: boolean;
  volumes?: { hostPath: string; containerPath: string; readOnly: boolean }[];
  cpuLimitMillicores?: number;
  memoryLimitBytes?: number;
  publicPort?: number;
};

export type DeployAppResult = {
  containerId: string;
  name: string;
  upstream: string;
  running: boolean;
};

export type AgentContainerPort = {
  ip?: string;
  privatePort: number;
  publicPort?: number;
  type: string;
};

export type AgentContainerMount = {
  type: string;
  name?: string;
  source: string;
  destination: string;
  readOnly: boolean;
};

export type AgentContainerSummary = {
  id: string;
  name: string;
  image: string;
  imageId: string;
  state: string;
  status: string;
  running: boolean;
  ports: AgentContainerPort[];
};

export type AgentContainerDetails = AgentContainerSummary & {
  env: string[];
  mounts: AgentContainerMount[];
};

export async function listImportableContainers(
  conn: AgentConnection,
  token: string,
): Promise<AgentContainerSummary[]> {
  const response = await getJson<{ containers: AgentContainerSummary[] }>(
    conn,
    token,
    "/v1/apps/importable-containers",
    true,
  );
  return response.containers;
}

export async function importContainer(
  conn: AgentConnection,
  token: string,
  input: { appId: string; containerId: string },
): Promise<AgentContainerDetails> {
  return postJson<AgentContainerDetails>(conn, token, "/v1/apps/import-container", input, 60_000);
}

/**
 * Deploys an app on the server: pulls the (private Depot) image and runs the
 * container on the 'basse' network. The image pull dominates, so the inner
 * timeout is generous. Throws on failure.
 */
export async function deployApp(
  conn: AgentConnection,
  token: string,
  input: DeployAppInput,
): Promise<DeployAppResult> {
  return postJson<DeployAppResult>(conn, token, "/v1/apps/deploy", input, 300_000);
}

/** Reports whether an app container exists and is running. Throws on failure. */
export async function getAppStatus(
  conn: AgentConnection,
  token: string,
  appId: string,
): Promise<{ exists: boolean; running: boolean }> {
  return getJson<{ exists: boolean; running: boolean }>(
    conn,
    token,
    `/v1/apps/${appId}/status`,
    true,
  );
}

export type AgentAppMetrics = {
  cpuPercent: number;
  memoryBytes: number;
  memoryLimitBytes: number;
  memoryPercent: number;
};

export async function getAppMetrics(
  conn: AgentConnection,
  token: string,
  appId: string,
): Promise<AgentAppMetrics> {
  return getJson<AgentAppMetrics>(conn, token, `/v1/apps/${appId}/metrics`, true);
}

export async function getAppLogs(
  conn: AgentConnection,
  token: string,
  appId: string,
  tail = 250,
): Promise<{ logs: string }> {
  return getJson<{ logs: string }>(
    conn,
    token,
    `/v1/apps/${appId}/logs?tail=${encodeURIComponent(String(tail))}`,
    true,
  );
}

export async function execAppCommand(
  conn: AgentConnection,
  token: string,
  appId: string,
  command: string,
): Promise<{ exitCode: number; output: string }> {
  return postJson<{ exitCode: number; output: string }>(
    conn,
    token,
    `/v1/apps/${appId}/exec`,
    { command },
    25_000,
  );
}

export type BackupTargetInput = {
  backupId: string;
  database: string;
  user: string;
  dataDir: string;
};

/** Runs pg_dump inside the database container; the dump lands on its data volume. */
export async function createAgentBackup(
  conn: AgentConnection,
  token: string,
  appId: string,
  input: BackupTargetInput,
): Promise<{ sizeBytes: number }> {
  return postJson<{ sizeBytes: number }>(
    conn,
    token,
    `/v1/apps/${appId}/backups`,
    input,
    1_800_000,
  );
}

/** Runs pg_restore --clean --if-exists from an existing dump. Throws on failure. */
export async function restoreAgentBackup(
  conn: AgentConnection,
  token: string,
  appId: string,
  input: BackupTargetInput,
): Promise<void> {
  await postJson(
    conn,
    token,
    `/v1/apps/${appId}/backups/${input.backupId}/restore`,
    input,
    1_800_000,
  );
}

/** Removes a dump file from the database's data volume. Missing files succeed. */
export async function deleteAgentBackup(
  conn: AgentConnection,
  token: string,
  appId: string,
  backupId: string,
  dataDir: string,
): Promise<void> {
  const response = await fetchAgent(conn, token, {
    method: "DELETE",
    path: `/v1/apps/${appId}/backups/${backupId}?dataDir=${encodeURIComponent(dataDir)}`,
    authed: true,
    timeoutMs: 30_000,
  });
  if (!response.ok) {
    throw new Error(`agent backup delete returned ${response.status}`);
  }
}

/** Tears down an app container. Throws on failure. */
export async function removeApp(
  conn: AgentConnection,
  token: string,
  appId: string,
): Promise<void> {
  const response = await fetchAgent(conn, token, {
    method: "DELETE",
    path: `/v1/apps/${appId}`,
    authed: true,
    timeoutMs: 30_000,
  });
  if (!response.ok) {
    throw new Error(`agent app remove returned ${response.status}`);
  }
}

/**
 * Opens a tunnel to the agent and checks liveness, readiness, and version.
 * Retries with backoff (the agent may still be starting right after bootstrap).
 * Never throws — returns a structured health result.
 */
export async function checkAgentHealth(
  conn: AgentConnection,
  token: string,
  options: { attempts?: number } = {},
): Promise<AgentHealth> {
  const attempts = options.attempts ?? 5;

  try {
    let lastError = "";

    for (let attempt = 0; attempt < attempts; attempt++) {
      try {
        await getJson<{ status: string }>(conn, token, "/healthz", false);

        let ready = false;
        try {
          await getJson<{ status: string }>(conn, token, "/readyz", false);
          ready = true;
        } catch {
          ready = false;
        }

        const version = await getJson<AgentVersion>(conn, token, "/v1/version", true);
        return { reachable: true, ready, version: version.version };
      } catch (error) {
        lastError = error instanceof Error ? error.message : String(error);
        await Bun.sleep(1000 * (attempt + 1));
      }
    }

    return { reachable: false, ready: false, error: lastError || "agent unreachable" };
  } catch (error) {
    return {
      reachable: false,
      ready: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
