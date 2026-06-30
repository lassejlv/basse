import { db, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { AGENT_PORT, checkAgentHealth, ensureProxy } from "./agent-client";
import { decryptSecret, encryptSecret } from "./crypto";
import { syncServerDomains } from "./proxy-sync";
import { connectionFromServer } from "./server-connection";
import { probeReachable, runScript, writeRemoteFile } from "./ssh";

type ServerRow = typeof server.$inferSelect;

const AGENT_IMAGE = Bun.env.BASSE_AGENT_IMAGE ?? "ghcr.io/lassejlv/basse-agent:latest";
const CADDY_IMAGE = Bun.env.BASSE_CADDY_IMAGE ?? "caddy:2";
const AGENT_ENV_PATH = "/etc/basse/agent.env";

// The admin unix socket lives on this named volume, mounted into BOTH the agent
// (at agent-run time, here) and the Caddy container (by the agent). Must match
// the agent's BASSE_CADDY_ADMIN_VOLUME / BASSE_CADDY_ADMIN_DIR defaults.
const CADDY_ADMIN_MOUNT = "basse_caddy_admin:/run/caddy-admin";

// Operator-controlled image ref; validate defensively since it is the one value
// interpolated into the remote bootstrap script.
const IMAGE_REF_PATTERN = /^[A-Za-z0-9._/:@-]+$/;

async function setStatus(
  id: string,
  status: ServerRow["status"],
  statusMessage: string | null,
  extra: Partial<Pick<ServerRow, "agentUrl" | "lastSeenAt" | "hostKeyFingerprint">> = {},
): Promise<void> {
  await db
    .update(server)
    .set({ status, statusMessage, updatedAt: new Date(), ...extra })
    .where(eq(server.id, id));
}

/** A 32-byte base64url bearer token for the agent. */
function generateAgentToken(): string {
  const bytes = crypto.getRandomValues(new Uint8Array(32));
  return Buffer.from(bytes).toString("base64url");
}

/** Ensures the server has an agent token; returns the plaintext. Idempotent. */
async function ensureAgentToken(row: ServerRow): Promise<string> {
  if (row.agentToken) {
    return decryptSecret(row.agentToken);
  }

  const token = generateAgentToken();
  const encrypted = await encryptSecret(token);
  await db.update(server).set({ agentToken: encrypted }).where(eq(server.id, row.id));
  return token;
}

function bootstrapScript(image: string): string {
  return `set -euo pipefail
if ! command -v docker >/dev/null 2>&1; then
  echo "Installing Docker..."
  curl -fsSL https://get.docker.com | sh
fi
echo "Verifying Docker..."
docker info >/dev/null 2>&1
echo "Pulling agent image..."
docker pull "${image}"
echo "Starting agent..."
docker rm -f basse-agent >/dev/null 2>&1 || true
docker run -d --name basse-agent --restart unless-stopped \\
  -p 127.0.0.1:${AGENT_PORT}:${AGENT_PORT} \\
  -v /var/run/docker.sock:/var/run/docker.sock \\
  -v ${CADDY_ADMIN_MOUNT} \\
  --env-file ${AGENT_ENV_PATH} \\
  "${image}"
echo "Agent container started."
`;
}

/**
 * Provisions a server: SSH in, install Docker if missing, run the Go agent, and
 * health-check it through an SSH tunnel. Idempotent and NEVER throws — every
 * failure path lands the server in a terminal status with a message. The caller
 * must already have claimed the row (status='provisioning').
 */
export async function provisionServer(serverId: string): Promise<void> {
  try {
    const [row] = await db.select().from(server).where(eq(server.id, serverId)).limit(1);

    if (!row) {
      return;
    }

    if (!IMAGE_REF_PATTERN.test(AGENT_IMAGE)) {
      await setStatus(serverId, "error", "Invalid agent image reference");
      return;
    }

    const token = await ensureAgentToken(row);
    const connection = await connectionFromServer(row);

    await setStatus(serverId, "provisioning", "Connecting over SSH…");
    const probe = await probeReachable(connection);

    if (probe.fingerprint) {
      await db
        .update(server)
        .set({ hostKeyFingerprint: probe.fingerprint })
        .where(eq(server.id, serverId));
    }

    if (!probe.ok) {
      await setStatus(serverId, "error", `SSH connection failed: ${probe.error ?? "unreachable"}`);
      return;
    }

    await setStatus(serverId, "provisioning", "Writing agent configuration…");
    await writeRemoteFile(
      connection,
      AGENT_ENV_PATH,
      `BASSE_AGENT_TOKEN=${token}\nBASSE_AGENT_PORT=${AGENT_PORT}\nBASSE_CADDY_IMAGE=${CADDY_IMAGE}\n`,
    );

    await setStatus(serverId, "provisioning", "Installing Docker and starting agent…");
    const result = await runScript(connection, bootstrapScript(AGENT_IMAGE), {
      onLine: (line) => {
        const trimmed = line.trim();
        if (trimmed) {
          void setStatus(serverId, "provisioning", trimmed);
        }
      },
    });

    if (result.exitCode !== 0) {
      const tail = result.output.trim().split("\n").slice(-3).join(" ");
      await setStatus(serverId, "error", `Bootstrap failed: ${tail || `exit ${result.exitCode}`}`);
      return;
    }

    await setStatus(serverId, "provisioning", "Waiting for the agent to become healthy…");
    const health = await checkAgentHealth(connection, token);

    if (!health.reachable || !health.ready) {
      await setStatus(
        serverId,
        "unreachable",
        health.error ?? "Agent started but did not become healthy",
      );
      return;
    }

    // Bring up the Caddy proxy and push the current desired domain set. A proxy
    // failure (e.g. :80/:443 already bound) is terminal — the server is not ready.
    await setStatus(serverId, "provisioning", "Starting the proxy…");
    try {
      await ensureProxy(connection, token);
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      await setStatus(serverId, "error", `Proxy setup failed: ${message}`);
      return;
    }

    await setStatus(serverId, "provisioning", "Applying domains…");
    await syncServerDomains(serverId);

    await setStatus(serverId, "active", null, {
      agentUrl: `http://127.0.0.1:${AGENT_PORT}`,
      lastSeenAt: new Date(),
    });
  } catch (error) {
    const message = error instanceof Error ? error.message : String(error);
    await setStatus(serverId, "error", `Provisioning error: ${message}`).catch(() => {});
  }
}
