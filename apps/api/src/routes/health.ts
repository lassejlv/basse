import type { app } from "@basse/db";
import { execAppCommand, type AgentConnection } from "../infra/agent-client";

// HTTP health probes run INSIDE the app container via agent exec (curl, then
// wget as a fallback), because the agent container is not on the app network.
// Same tradeoff Coolify documents: UI health checks need curl or wget in the
// image. Containers with neither report "no probe tool" rather than unhealthy.

type AppRow = typeof app.$inferSelect;

export type HealthProbeResult =
  | { ok: true; status: number }
  | { ok: false; status: number | null; reason: string };

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

/**
 * One HTTP probe against localhost inside the container. curl reports the
 * exact status code; busybox wget can only distinguish success (2xx) from
 * failure, so under wget a non-2xx expectation degrades to "reachable".
 */
export async function probeAppHttp(
  connection: AgentConnection,
  agentToken: string,
  appRow: Pick<
    AppRow,
    "id" | "port" | "healthCheckPath" | "healthCheckStatus" | "healthCheckTimeoutSeconds"
  >,
): Promise<HealthProbeResult> {
  const url = `http://127.0.0.1:${appRow.port}${appRow.healthCheckPath}`;
  const timeout = Math.min(Math.max(appRow.healthCheckTimeoutSeconds, 1), 60);
  const command = `
if command -v curl >/dev/null 2>&1; then
  code=$(curl -s -o /dev/null -w '%{http_code}' --max-time ${timeout} ${shellQuote(url)} 2>/dev/null)
  echo "CODE:$code"
elif command -v wget >/dev/null 2>&1; then
  if wget -q -T ${timeout} -O /dev/null ${shellQuote(url)} 2>/dev/null; then
    echo "CODE:200"
  else
    echo "CODE:000"
  fi
else
  echo "NO_PROBE_TOOL"
fi
`.trim();

  const result = await execAppCommand(connection, agentToken, appRow.id, command);
  const output = result.output.trim();

  if (output.includes("NO_PROBE_TOOL")) {
    return {
      ok: false,
      status: null,
      reason: "Neither curl nor wget is available in the container image",
    };
  }
  const match = output.match(/CODE:(\d{3})/);
  if (!match) {
    return { ok: false, status: null, reason: output || "Health probe produced no status" };
  }
  const status = Number(match[1]);
  if (status === 0) {
    return { ok: false, status: null, reason: "Connection refused or timed out" };
  }
  if (status === appRow.healthCheckStatus) {
    return { ok: true, status };
  }
  return {
    ok: false,
    status,
    reason: `Expected status ${appRow.healthCheckStatus}, got ${status}`,
  };
}

/**
 * Deploy-time gate: polls the health check until it passes or the budget runs
 * out. A missing probe tool fails fast instead of burning the whole budget.
 */
export async function waitForHealthy(
  connection: AgentConnection,
  agentToken: string,
  appRow: Pick<
    AppRow,
    "id" | "port" | "healthCheckPath" | "healthCheckStatus" | "healthCheckTimeoutSeconds"
  >,
  options: { budgetMs?: number; onLine?: (line: string) => void } = {},
): Promise<HealthProbeResult> {
  const budgetMs = options.budgetMs ?? 60_000;
  const startedAt = Date.now();
  let last: HealthProbeResult = { ok: false, status: null, reason: "Not probed yet" };

  while (Date.now() - startedAt < budgetMs) {
    last = await probeAppHttp(connection, agentToken, appRow);
    if (last.ok) return last;
    if (!last.ok && last.status === null && last.reason.includes("curl nor wget")) {
      return last;
    }
    await Bun.sleep(3000);
  }
  return last;
}
