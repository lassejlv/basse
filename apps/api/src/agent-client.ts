import type { DesiredDomain, ProxyStatus } from "@basse/shared";
import type { SshConnection } from "./ssh";
import { withTunnel } from "./ssh";

// Talks to the on-server Go agent. The agent listens on the server's loopback
// only, so every call goes through an SSH local-port-forward (withTunnel). The
// bearer token therefore never crosses the public network.

export const AGENT_PORT = 8888;

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

type AgentCallContext = {
  baseUrl: string;
  token: string;
};

async function getJson<T>(ctx: AgentCallContext, path: string, authed: boolean): Promise<T> {
  const response = await fetch(`${ctx.baseUrl}${path}`, {
    headers: authed ? { authorization: `Bearer ${ctx.token}` } : undefined,
    signal: AbortSignal.timeout(10_000),
  });

  if (!response.ok) {
    throw new Error(`agent ${path} returned ${response.status}`);
  }

  return response.json() as Promise<T>;
}

async function postJson<T>(
  ctx: AgentCallContext,
  path: string,
  body: unknown,
  timeoutMs: number,
): Promise<T> {
  const response = await fetch(`${ctx.baseUrl}${path}`, {
    method: "POST",
    headers: {
      authorization: `Bearer ${ctx.token}`,
      "content-type": "application/json",
    },
    body: JSON.stringify(body),
    signal: AbortSignal.timeout(timeoutMs),
  });

  if (!response.ok) {
    const detail = (await response.json().catch(() => null)) as { error?: string } | null;
    throw new Error(detail?.error ?? `agent ${path} returned ${response.status}`);
  }

  return response.json() as Promise<T>;
}

/**
 * Brings up Caddy on the server (idempotent). Opens a tunnel and calls
 * /v1/proxy/ensure with a generous timeout (image pull + container start +
 * admin wait). Throws on failure.
 */
export async function ensureProxy(conn: SshConnection, token: string): Promise<ProxyStatus> {
  return withTunnel(
    conn,
    AGENT_PORT,
    (baseUrl) => postJson<ProxyStatus>({ baseUrl, token }, "/v1/proxy/ensure", {}, 170_000),
    { timeoutMs: 20_000 },
  );
}

export async function getAgentInfo(conn: SshConnection, token: string): Promise<AgentInfo> {
  return withTunnel(conn, AGENT_PORT, (baseUrl) =>
    getJson<AgentInfo>({ baseUrl, token }, "/v1/info", true),
  );
}

/** Pushes the full desired domain set to the server's Caddy. Throws on failure. */
export async function syncDomains(
  conn: SshConnection,
  token: string,
  domains: DesiredDomain[],
): Promise<void> {
  await withTunnel(conn, AGENT_PORT, (baseUrl) =>
    postJson({ baseUrl, token }, "/v1/proxy/sync", { domains }, 30_000),
  );
}

export type DeployAppInput = {
  appId: string;
  image: string;
  port: number;
  env: Record<string, string>;
  registry: { host: string; user: string; token: string };
};

export type DeployAppResult = {
  containerId: string;
  name: string;
  upstream: string;
  running: boolean;
};

/**
 * Deploys an app on the server: pulls the (private Depot) image and runs the
 * container on the 'basse' network. The image pull dominates, so the inner
 * timeout is generous. Throws on failure.
 */
export async function deployApp(
  conn: SshConnection,
  token: string,
  input: DeployAppInput,
): Promise<DeployAppResult> {
  return withTunnel(
    conn,
    AGENT_PORT,
    (baseUrl) => postJson<DeployAppResult>({ baseUrl, token }, "/v1/apps/deploy", input, 300_000),
    { timeoutMs: 20_000 },
  );
}

/** Reports whether an app container exists and is running. Throws on failure. */
export async function getAppStatus(
  conn: SshConnection,
  token: string,
  appId: string,
): Promise<{ exists: boolean; running: boolean }> {
  return withTunnel(conn, AGENT_PORT, (baseUrl) =>
    getJson<{ exists: boolean; running: boolean }>({ baseUrl, token }, `/v1/apps/${appId}/status`, true),
  );
}

export type AgentAppMetrics = {
  cpuPercent: number;
  memoryBytes: number;
  memoryLimitBytes: number;
  memoryPercent: number;
};

export async function getAppMetrics(
  conn: SshConnection,
  token: string,
  appId: string,
): Promise<AgentAppMetrics> {
  return withTunnel(conn, AGENT_PORT, (baseUrl) =>
    getJson<AgentAppMetrics>({ baseUrl, token }, `/v1/apps/${appId}/metrics`, true),
  );
}

export async function execAppCommand(
  conn: SshConnection,
  token: string,
  appId: string,
  command: string,
): Promise<{ exitCode: number; output: string }> {
  return withTunnel(
    conn,
    AGENT_PORT,
    (baseUrl) =>
      postJson<{ exitCode: number; output: string }>(
        { baseUrl, token },
        `/v1/apps/${appId}/exec`,
        { command },
        25_000,
      ),
    { timeoutMs: 20_000 },
  );
}

/** Tears down an app container. Throws on failure. */
export async function removeApp(conn: SshConnection, token: string, appId: string): Promise<void> {
  await withTunnel(conn, AGENT_PORT, async (baseUrl) => {
    const response = await fetch(`${baseUrl}/v1/apps/${appId}`, {
      method: "DELETE",
      headers: { authorization: `Bearer ${token}` },
      signal: AbortSignal.timeout(30_000),
    });
    if (!response.ok) {
      throw new Error(`agent app remove returned ${response.status}`);
    }
  });
}

/**
 * Opens a tunnel to the agent and checks liveness, readiness, and version.
 * Retries with backoff (the agent may still be starting right after bootstrap).
 * Never throws — returns a structured health result.
 */
export async function checkAgentHealth(
  conn: SshConnection,
  token: string,
  options: { attempts?: number } = {},
): Promise<AgentHealth> {
  const attempts = options.attempts ?? 5;

  try {
    return await withTunnel(conn, AGENT_PORT, async (baseUrl) => {
      const ctx: AgentCallContext = { baseUrl, token };
      let lastError = "";

      for (let attempt = 0; attempt < attempts; attempt++) {
        try {
          await getJson<{ status: string }>(ctx, "/healthz", false);

          let ready = false;
          try {
            await getJson<{ status: string }>(ctx, "/readyz", false);
            ready = true;
          } catch {
            ready = false;
          }

          const version = await getJson<AgentVersion>(ctx, "/v1/version", true);
          return { reachable: true, ready, version: version.version };
        } catch (error) {
          lastError = error instanceof Error ? error.message : String(error);
          await Bun.sleep(1000 * (attempt + 1));
        }
      }

      return { reachable: false, ready: false, error: lastError || "agent unreachable" };
    });
  } catch (error) {
    return {
      reachable: false,
      ready: false,
      error: error instanceof Error ? error.message : String(error),
    };
  }
}
