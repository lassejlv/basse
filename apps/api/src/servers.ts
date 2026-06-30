import { db, server } from "@basse/db";
import type {
  AgentInfo,
  AgentLogs,
  AgentMetrics,
  AgentUpdateCheck,
  CreateServerInput,
  Server,
} from "@basse/shared";
import { and, eq, ne } from "drizzle-orm";
import { Hono } from "hono";
import { checkAgentHealth, getAgentInfo } from "./agent-client";
import { decryptSecret, encryptSecret } from "./crypto";
import { AGENT_IMAGE, IMAGE_REF_PATTERN } from "./provision";
import { enqueueAction } from "./queue/queue";
import { connectionFromServer } from "./server-connection";
import { derivePublicKey, generateServerKeyPair } from "./server-keys";
import { probeReachable, runScript } from "./ssh";
import { resolveActiveWorkspace } from "./workspace";

type ServerRow = typeof server.$inferSelect;

/**
 * Maps a DB row to the client-facing DTO. Never exposes the private key or the
 * raw agent token — only the public key and a last-4 token hint (depot pattern).
 */
async function sanitizeServer(row: ServerRow): Promise<Server> {
  let agentTokenHint: string | undefined;

  if (row.agentToken) {
    try {
      const token = await decryptSecret(row.agentToken);
      agentTokenHint = token.slice(-4);
    } catch {
      agentTokenHint = undefined;
    }
  }

  return {
    id: row.id,
    organizationId: row.organizationId,
    name: row.name,
    sshHost: row.sshHost,
    sshPort: row.sshPort,
    sshUser: row.sshUser,
    sshPublicKey: row.sshPublicKey,
    agentUrl: row.agentUrl,
    status: row.status,
    statusMessage: row.statusMessage,
    hostKeyFingerprint: row.hostKeyFingerprint,
    agentTokenHint,
    lastSeenAt: row.lastSeenAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

export const servers = new Hono();

async function ownedServer(serverId: string, organizationId: string): Promise<ServerRow | null> {
  const [row] = await db
    .select()
    .from(server)
    .where(and(eq(server.id, serverId), eq(server.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
}

async function requireAgent(row: ServerRow) {
  if (!row.agentToken) {
    return { error: "Server has not been provisioned yet" };
  }
  const connection = await connectionFromServer(row);
  const token = await decryptSecret(row.agentToken);
  return { connection, token };
}

function parseDockerStatsLine(line: string): Omit<AgentMetrics, "timestamp"> {
  const trimmed = line.trim();
  if (!trimmed) {
    throw new Error("docker stats returned no data");
  }
  const parsed = JSON.parse(trimmed) as {
    CPUPerc?: string;
    MemUsage?: string;
    MemPerc?: string;
  };
  const cpuPercent = Number((parsed.CPUPerc ?? "0").replace("%", ""));
  const memoryPercent = Number((parsed.MemPerc ?? "0").replace("%", ""));
  const [memoryValue, memoryLimit] = (parsed.MemUsage ?? "0B / 0B").split("/").map((v) => v.trim());
  return {
    cpuPercent: Number.isFinite(cpuPercent) ? cpuPercent : 0,
    memoryBytes: parseByteSize(memoryValue ?? "0B"),
    memoryLimitBytes: parseByteSize(memoryLimit ?? "0B"),
    memoryPercent: Number.isFinite(memoryPercent) ? memoryPercent : 0,
  };
}

function parseByteSize(value: string): number {
  const match = value.match(/^([\d.]+)\s*([KMGT]?i?B|B)$/i);
  if (!match) return 0;
  const amount = Number(match[1]);
  const unit = match[2]?.toLowerCase() ?? "b";
  const scale =
    unit === "kib" || unit === "kb"
      ? 1024
      : unit === "mib" || unit === "mb"
        ? 1024 ** 2
        : unit === "gib" || unit === "gb"
          ? 1024 ** 3
          : unit === "tib" || unit === "tb"
            ? 1024 ** 4
            : 1;
  return Number.isFinite(amount) ? Math.round(amount * scale) : 0;
}

function imageIdFromOutput(output: string, key: string): string | null {
  const line = output
    .split("\n")
    .find((candidate) => candidate.startsWith(`${key}=`));
  const value = line?.slice(key.length + 1).trim();
  return value || null;
}

servers.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const rows = await db
    .select()
    .from(server)
    .where(eq(server.organizationId, organizationId))
    .orderBy(server.createdAt);

  return c.json(await Promise.all(rows.map(sanitizeServer)));
});

servers.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select()
    .from(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }

  return c.json(await sanitizeServer(row));
});

servers.get("/:id/agent", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);

  const agent = await requireAgent(row);
  if (!agent.connection) {
    return c.json({
      reachable: false,
      ready: false,
      targetImage: AGENT_IMAGE,
      error: agent.error,
    } satisfies AgentInfo);
  }

  const health = await checkAgentHealth(agent.connection, agent.token, { attempts: 1 });
  if (!health.reachable || !health.ready) {
    return c.json({
      reachable: health.reachable,
      ready: health.ready,
      version: health.version,
      targetImage: AGENT_IMAGE,
      error: health.error,
    } satisfies AgentInfo);
  }

  try {
    const info = await getAgentInfo(agent.connection, agent.token);
    return c.json({
      reachable: true,
      ready: true,
      version: info.agent.version,
      targetImage: AGENT_IMAGE,
      docker: {
        containers: info.docker.Containers,
        containersRunning: info.docker.ContainersRunning,
        images: info.docker.Images,
        ncpu: info.docker.NCPU,
        memTotal: info.docker.MemTotal,
      },
      engine: {
        version: info.engine.Version,
        apiVersion: info.engine.ApiVersion,
        os: info.engine.Os,
        arch: info.engine.Arch,
      },
    } satisfies AgentInfo);
  } catch (error) {
    return c.json({
      reachable: true,
      ready: false,
      targetImage: AGENT_IMAGE,
      error: error instanceof Error ? error.message : String(error),
    } satisfies AgentInfo);
  }
});

servers.get("/:id/agent/logs", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);

  const tail = Math.min(Math.max(Number(c.req.query("tail") ?? 200) || 200, 20), 1000);
  const result = await runScript(
    await connectionFromServer(row),
    `docker logs --tail ${tail} basse-agent 2>&1 || true`,
    { timeoutMs: 30_000 },
  );
  return c.json({ logs: result.output } satisfies AgentLogs);
});

servers.get("/:id/agent/metrics", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);

  const result = await runScript(
    await connectionFromServer(row),
    `docker stats --no-stream --format '{{json .}}' basse-agent`,
    { timeoutMs: 30_000 },
  );
  if (result.exitCode !== 0) {
    return c.json({ error: "Could not read agent metrics" }, 502);
  }
  return c.json({
    timestamp: new Date().toISOString(),
    ...parseDockerStatsLine(result.output),
  } satisfies AgentMetrics);
});

servers.post("/:id/agent/check-update", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (!IMAGE_REF_PATTERN.test(AGENT_IMAGE)) {
    return c.json({ error: "Invalid agent image reference" }, 400);
  }

  const result = await runScript(
    await connectionFromServer(row),
    `set -e
current="$(docker inspect --format '{{.Image}}' basse-agent 2>/dev/null || true)"
docker pull "${AGENT_IMAGE}"
latest="$(docker image inspect --format '{{.Id}}' "${AGENT_IMAGE}")"
echo "CURRENT=$current"
echo "LATEST=$latest"
`,
    { timeoutMs: 180_000 },
  );

  if (result.exitCode !== 0) {
    return c.json({ error: "Could not check for updates", output: result.output }, 502);
  }

  const currentImageId = imageIdFromOutput(result.output, "CURRENT");
  const latestImageId = imageIdFromOutput(result.output, "LATEST");
  return c.json({
    targetImage: AGENT_IMAGE,
    currentImageId,
    latestImageId,
    updateAvailable: Boolean(currentImageId && latestImageId && currentImageId !== latestImageId),
    output: result.output,
  } satisfies AgentUpdateCheck);
});

servers.post("/:id/agent/update", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const id = c.req.param("id");
  const row = await ownedServer(id, organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);

  await db
    .update(server)
    .set({ status: "provisioning", statusMessage: "Queued agent update…", updatedAt: new Date() })
    .where(eq(server.id, id));

  try {
    await enqueueAction("provision-server", id);
  } catch {
    await db
      .update(server)
      .set({
        status: "error",
        statusMessage: "Could not queue agent update (queue unavailable). Retry.",
        updatedAt: new Date(),
      })
      .where(eq(server.id, id));
    return c.json({ error: "Could not queue agent update" }, 503);
  }

  return c.body(null, 202);
});

servers.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateServerInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const sshHost = typeof body?.sshHost === "string" ? body.sshHost.trim() : "";
  const sshPort = typeof body?.sshPort === "number" ? body.sshPort : 22;
  const sshUser = typeof body?.sshUser === "string" && body.sshUser.trim() ? body.sshUser.trim() : "root";
  const providedPrivateKey =
    typeof body?.privateKey === "string" && body.privateKey.trim() ? body.privateKey.trim() : null;

  if (!name) {
    return c.json({ error: "name is required" }, 400);
  }

  if (!sshHost) {
    return c.json({ error: "sshHost is required" }, 400);
  }

  if (!Number.isInteger(sshPort) || sshPort < 1 || sshPort > 65535) {
    return c.json({ error: "sshPort must be a valid port" }, 400);
  }

  const id = crypto.randomUUID();

  // Either reuse a pasted private key (deriving its public half) or generate a
  // new per-server keypair.
  let publicKey: string;
  let privateKey: string;

  if (providedPrivateKey) {
    try {
      publicKey = await derivePublicKey(providedPrivateKey);
      privateKey = providedPrivateKey;
    } catch (error) {
      return c.json(
        { error: error instanceof Error ? error.message : "Invalid private key" },
        400,
      );
    }
  } else {
    const keyPair = await generateServerKeyPair(id);
    publicKey = keyPair.publicKey;
    privateKey = keyPair.privateKey;
  }

  const encryptedPrivateKey = await encryptSecret(privateKey);
  const now = new Date();

  const [created] = await db
    .insert(server)
    .values({
      id,
      organizationId,
      name,
      sshHost,
      sshPort,
      sshUser,
      sshPublicKey: publicKey,
      sshPrivateKey: encryptedPrivateKey,
      status: "pending",
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) {
    return c.json({ error: "Failed to create server" }, 500);
  }

  return c.json(await sanitizeServer(created), 201);
});

servers.post("/:id/provision", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const id = c.req.param("id");

  // Atomically claim the row: only a server that is not already provisioning can
  // be (re-)provisioned. The conditional update is the concurrency lock.
  const claimed = await db
    .update(server)
    .set({ status: "provisioning", statusMessage: "Queued…", updatedAt: new Date() })
    .where(
      and(
        eq(server.id, id),
        eq(server.organizationId, organizationId),
        ne(server.status, "provisioning"),
      ),
    )
    .returning({ id: server.id });

  if (!claimed[0]) {
    // Either it does not exist/belong to the workspace, or it is already running.
    const [row] = await db
      .select({ id: server.id })
      .from(server)
      .where(and(eq(server.id, id), eq(server.organizationId, organizationId)))
      .limit(1);

    if (!row) {
      return c.json({ error: "Server not found" }, 404);
    }

    return c.json({ error: "Server is already provisioning" }, 409);
  }

  // Enqueue the durable job. The worker runs provisionServer (which owns all
  // status writes). If Redis is unreachable, revert the claim so the row isn't
  // stuck on "Queued…" and the user can retry.
  try {
    await enqueueAction("provision-server", id);
  } catch {
    await db
      .update(server)
      .set({
        status: "error",
        statusMessage: "Could not queue provisioning (queue unavailable). Retry.",
        updatedAt: new Date(),
      })
      .where(eq(server.id, id));

    return c.json({ error: "Could not queue provisioning" }, 503);
  }

  return c.body(null, 202);
});

servers.post("/:id/proxy/resync", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select({ id: server.id })
    .from(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }

  try {
    await enqueueAction("sync-domains", row.id);
  } catch {
    return c.json({ error: "Could not queue resync" }, 503);
  }

  return c.body(null, 202);
});

servers.post("/:id/check-connection", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [row] = await db
    .select()
    .from(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .limit(1);

  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }

  const connection = await connectionFromServer(row);
  const result = await probeReachable(connection);

  if (result.fingerprint && result.fingerprint !== row.hostKeyFingerprint) {
    await db
      .update(server)
      .set({ hostKeyFingerprint: result.fingerprint, updatedAt: new Date() })
      .where(eq(server.id, row.id));
  }

  return c.json({
    ok: result.ok,
    fingerprint: result.fingerprint,
    error: result.error,
  });
});

servers.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const [deleted] = await db
    .delete(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, organizationId)))
    .returning({ id: server.id });

  if (!deleted) {
    return c.json({ error: "Server not found" }, 404);
  }

  return c.body(null, 204);
});
