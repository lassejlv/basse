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
