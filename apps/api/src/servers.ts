import { db, domain, member, server, sshKey, verification } from "@basse/db";
import type {
  AgentInfo,
  AgentLogs,
  AgentMetrics,
  AgentUpdateCheck,
  CreateServerInput,
  Server,
  ServerInstallCommand,
} from "@basse/shared";
import { and, eq, ne } from "drizzle-orm";
import { Hono } from "hono";
import { checkAgentHealth, getAgentInfo } from "./agent-client";
import { auth } from "./auth";
import { decryptSecret, encryptSecret } from "./crypto";
import { sendServerDeleteCodeEmail } from "./email";
import { agentTokenHash } from "./outbound-agent";
import { AGENT_IMAGE, IMAGE_REF_PATTERN } from "./provision";
import { enqueueOrRunDomainSync } from "./proxy-sync";
import { enqueueAction } from "./queue/queue";
import { connectionFromServer } from "./server-connection";
import { derivePublicKey, generateServerKeyPair } from "./server-keys";
import { probeReachable, runScript, type SshConnection } from "./ssh";
import { resolveActiveWorkspace } from "./workspace";

type ServerRow = typeof server.$inferSelect;
type ActiveWorkspaceUser = {
  organizationId: string;
  userId: string;
  email: string;
};

const SERVER_DELETE_CODE_TTL_MS = 10 * 60 * 1000;

/**
 * Maps a DB row to the client-facing DTO. Never exposes the private key or the
 * raw agent token — only the public key and a last-4 token hint (depot pattern).
 */
async function sanitizeServer(
  row: ServerRow,
  extras: Partial<Pick<Server, "agentInstallCommand">> = {},
): Promise<Server> {
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
    connectionMode: row.connectionMode,
    agentUrl: row.agentUrl,
    isSystem: row.isSystem,
    status: row.status,
    statusMessage: row.statusMessage,
    hostKeyFingerprint: row.hostKeyFingerprint,
    agentTokenHint,
    lastSeenAt: row.lastSeenAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
    ...extras,
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

async function resolveActiveWorkspaceUser(
  headers: Headers,
): Promise<ActiveWorkspaceUser | Response> {
  const session = await auth.api.getSession({ headers });

  if (!session) {
    return new Response("Unauthorized", { status: 401 });
  }

  const organizationId = session.session.activeOrganizationId;
  if (!organizationId) {
    return new Response("No active workspace", { status: 400 });
  }

  const [membership] = await db
    .select({ id: member.id })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.userId, session.user.id)))
    .limit(1);

  if (!membership) {
    return new Response("Forbidden", { status: 403 });
  }

  return {
    organizationId,
    userId: session.user.id,
    email: session.user.email,
  };
}

async function requireAgent(row: ServerRow) {
  if (!row.agentToken) {
    return { error: "Server has not been provisioned yet" };
  }
  const connection = await connectionFromServer(row);
  const token = await decryptSecret(row.agentToken);
  return { connection, token };
}

function isSshConnection(
  connection: Awaited<ReturnType<typeof connectionFromServer>>,
): connection is SshConnection {
  return !("mode" in connection);
}

function apiOriginFromRequest(request: Request): string {
  const configured = Bun.env.API_ORIGIN ?? Bun.env.BETTER_AUTH_URL;
  if (configured) return configured.replace(/\/+$/, "");
  return new URL(request.url).origin;
}

function outboundInstallCommand(input: { apiOrigin: string; token: string }): string {
  const installUrl =
    Bun.env.BASSE_AGENT_INSTALL_URL ??
    "https://raw.githubusercontent.com/lassejlv/basse/main/apps/agent/install.sh";
  return [
    `curl -fsSL ${shellArg(installUrl)} |`,
    `BASSE_AGENT_TOKEN=${shellArg(input.token)}`,
    `BASSE_CONTROL_PLANE_URL=${shellArg(input.apiOrigin)}`,
    `BASSE_AGENT_IMAGE=${shellArg(AGENT_IMAGE)}`,
    "sh",
  ].join(" \\\n  ");
}

function shellArg(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function parseDockerStatsLine(line: string): Omit<AgentMetrics, "timestamp"> {
  const trimmed = line.trim();
  if (!trimmed) {
    return emptyAgentMetrics();
  }
  const jsonLine = trimmed
    .split("\n")
    .map((candidate) => candidate.trim())
    .find((candidate) => candidate.startsWith("{"));
  if (!jsonLine) return emptyAgentMetrics();

  let parsed: {
    CPUPerc?: string;
    MemUsage?: string;
    MemPerc?: string;
  };
  try {
    parsed = JSON.parse(jsonLine) as {
      CPUPerc?: string;
      MemUsage?: string;
      MemPerc?: string;
    };
  } catch {
    return emptyAgentMetrics();
  }
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

function emptyAgentMetrics(): Omit<AgentMetrics, "timestamp"> {
  return {
    cpuPercent: 0,
    memoryBytes: 0,
    memoryLimitBytes: 0,
    memoryPercent: 0,
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
  const line = output.split("\n").find((candidate) => candidate.startsWith(`${key}=`));
  const value = line?.slice(key.length + 1).trim();
  return value || null;
}

function deleteVerificationIdentifier(input: {
  organizationId: string;
  serverId: string;
  userId: string;
}): string {
  return `server-delete:${input.organizationId}:${input.serverId}:${input.userId}`;
}

function deleteCode(): string {
  const [value = 0] = crypto.getRandomValues(new Uint32Array(1));
  return String(value % 1_000_000).padStart(6, "0");
}

async function hashDeleteCode(code: string): Promise<string> {
  const secret = Bun.env.BETTER_AUTH_SECRET;
  if (!secret) throw new Error("BETTER_AUTH_SECRET is required");
  const bytes = new TextEncoder().encode(`${secret}:server-delete:${code}`);
  const digest = await crypto.subtle.digest("SHA-256", bytes);
  return Buffer.from(digest).toString("hex");
}

function normalizeDeleteCode(value: unknown): string {
  return typeof value === "string" ? value.trim().replace(/\s+/g, "") : "";
}

function isCloudRuntime(): boolean {
  return Object.keys(Bun.env).some((key) => key.startsWith("CLOUD_"));
}

function localServerId(organizationId: string): string {
  return `local-${organizationId}`;
}

async function ensureLocalServer(organizationId: string): Promise<void> {
  if (isCloudRuntime()) return;

  const rawAgentToken = Bun.env.BASSE_LOCAL_AGENT_TOKEN?.trim();
  if (!rawAgentToken) {
    console.warn("[servers] BASSE_LOCAL_AGENT_TOKEN is not set; local server was not created");
    return;
  }

  const id = localServerId(organizationId);
  const now = new Date();
  const [existing] = await db.select().from(server).where(eq(server.id, id)).limit(1);
  const encryptedAgentToken = await encryptSecret(rawAgentToken);
  const hashedAgentToken = await agentTokenHash(rawAgentToken);

  if (existing) {
    await db
      .update(server)
      .set({
        name: "Local server",
        sshHost: "localhost",
        sshPort: 22,
        sshUser: "root",
        agentToken: encryptedAgentToken,
        agentTokenHash: hashedAgentToken,
        connectionMode: "outbound",
        agentUrl: `outbound:${id}`,
        isSystem: true,
        updatedAt: now,
      })
      .where(and(eq(server.id, id), eq(server.organizationId, organizationId)));
    return;
  }

  const keyPair = await generateServerKeyPair(id);
  await db.insert(server).values({
    id,
    organizationId,
    name: "Local server",
    sshHost: "localhost",
    sshPort: 22,
    sshUser: "root",
    sshPublicKey: keyPair.publicKey,
    sshPrivateKey: await encryptSecret(keyPair.privateKey),
    agentToken: encryptedAgentToken,
    agentTokenHash: hashedAgentToken,
    connectionMode: "outbound",
    agentUrl: `outbound:${id}`,
    isSystem: true,
    status: "pending",
    statusMessage: "Waiting for the local agent to connect…",
    createdAt: now,
    updatedAt: now,
  });
}

servers.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  await ensureLocalServer(organizationId);

  const rows = await db
    .select()
    .from(server)
    .where(eq(server.organizationId, organizationId))
    .orderBy(server.createdAt);

  return c.json(await Promise.all(rows.map((row) => sanitizeServer(row))));
});

servers.get("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode === "outbound") {
    return c.json({ error: "Outbound servers do not expose host-level agent logs over SSH" }, 400);
  }

  const tail = Math.min(Math.max(Number(c.req.query("tail") ?? 200) || 200, 20), 1000);
  const connection = await connectionFromServer(row);
  if (!isSshConnection(connection))
    return c.json({ error: "Server is not configured for SSH" }, 400);
  const result = await runScript(
    connection,
    `docker logs --tail ${tail} basse-agent 2>&1 || true`,
    { timeoutMs: 30_000 },
  );
  return c.json({ logs: result.output } satisfies AgentLogs);
});

servers.get("/:id/agent/metrics", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode === "outbound") {
    return c.json(
      { error: "Outbound servers do not expose host-level Docker stats over SSH" },
      400,
    );
  }

  const connection = await connectionFromServer(row);
  if (!isSshConnection(connection))
    return c.json({ error: "Server is not configured for SSH" }, 400);
  const result = await runScript(
    connection,
    `docker stats --no-stream --format '{{json .}}' basse-agent 2>/dev/null || true`,
    { timeoutMs: 30_000 },
  );
  return c.json({
    timestamp: new Date().toISOString(),
    ...parseDockerStatsLine(result.output),
  } satisfies AgentMetrics);
});

servers.post("/:id/agent/check-update", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode === "outbound") {
    return c.json({ error: "Outbound agent updates must be run on the server with Docker" }, 400);
  }
  if (!IMAGE_REF_PATTERN.test(AGENT_IMAGE)) {
    return c.json({ error: "Invalid agent image reference" }, 400);
  }

  const connection = await connectionFromServer(row);
  if (!isSshConnection(connection))
    return c.json({ error: "Server is not configured for SSH" }, 400);
  const result = await runScript(
    connection,
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
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const id = c.req.param("id");
  const row = await ownedServer(id, organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode === "outbound") {
    return c.json({ error: "Outbound servers are updated by rerunning the install command" }, 400);
  }

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
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const body = (await c.req.json().catch(() => null)) as Partial<CreateServerInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  const sshHost = typeof body?.sshHost === "string" ? body.sshHost.trim() : "";
  const sshPort = typeof body?.sshPort === "number" ? body.sshPort : 22;
  const sshUser =
    typeof body?.sshUser === "string" && body.sshUser.trim() ? body.sshUser.trim() : "root";
  const connectionMode = body?.connectionMode === "outbound" ? "outbound" : "ssh";
  const sshKeyId =
    typeof body?.sshKeyId === "string" && body.sshKeyId.trim() ? body.sshKeyId.trim() : null;
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

  if (connectionMode === "ssh" && sshKeyId && providedPrivateKey) {
    return c.json({ error: "Choose a saved SSH key or paste a private key, not both" }, 400);
  }

  const id = crypto.randomUUID();

  // Either reuse a pasted private key (deriving its public half) or generate a
  // new per-server keypair.
  let publicKey: string;
  let privateKey: string;

  if (connectionMode === "outbound") {
    const keyPair = await generateServerKeyPair(id);
    publicKey = keyPair.publicKey;
    privateKey = keyPair.privateKey;
  } else if (sshKeyId) {
    const [storedKey] = await db
      .select()
      .from(sshKey)
      .where(and(eq(sshKey.id, sshKeyId), eq(sshKey.organizationId, organizationId)))
      .limit(1);

    if (!storedKey) {
      return c.json({ error: "SSH key not found" }, 404);
    }

    if (!storedKey.privateKey) {
      return c.json({ error: "Selected SSH key does not have a stored private key" }, 400);
    }

    publicKey = storedKey.publicKey;
    privateKey = await decryptSecret(storedKey.privateKey);
  } else if (providedPrivateKey) {
    try {
      publicKey = await derivePublicKey(providedPrivateKey);
      privateKey = providedPrivateKey;
    } catch (error) {
      return c.json({ error: error instanceof Error ? error.message : "Invalid private key" }, 400);
    }
  } else {
    const keyPair = await generateServerKeyPair(id);
    publicKey = keyPair.publicKey;
    privateKey = keyPair.privateKey;
  }

  const encryptedPrivateKey = await encryptSecret(privateKey);
  const rawAgentToken =
    connectionMode === "outbound" ? `ba_${crypto.randomUUID().replaceAll("-", "")}` : null;
  const encryptedAgentToken = rawAgentToken ? await encryptSecret(rawAgentToken) : null;
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
      agentToken: encryptedAgentToken,
      agentTokenHash: rawAgentToken ? await agentTokenHash(rawAgentToken) : null,
      connectionMode,
      agentUrl: connectionMode === "outbound" ? `outbound:${id}` : null,
      status: "pending",
      statusMessage:
        connectionMode === "outbound" ? "Waiting for outbound agent to connect…" : null,
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) {
    return c.json({ error: "Failed to create server" }, 500);
  }

  return c.json(
    await sanitizeServer(
      created,
      rawAgentToken
        ? {
            agentInstallCommand: outboundInstallCommand({
              apiOrigin: apiOriginFromRequest(c.req.raw),
              token: rawAgentToken,
            }),
          }
        : {},
    ),
    201,
  );
});

servers.post("/:id/install-command", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const row = await ownedServer(c.req.param("id"), organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode !== "outbound") {
    return c.json({ error: "Install commands are only available for outbound servers" }, 400);
  }
  if (!row.agentToken) {
    return c.json({ error: "Outbound server is missing an agent token" }, 400);
  }

  const token = await decryptSecret(row.agentToken);
  return c.json({
    agentInstallCommand: outboundInstallCommand({
      apiOrigin: apiOriginFromRequest(c.req.raw),
      token,
    }),
  } satisfies ServerInstallCommand);
});

servers.post("/:id/provision", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

  if (organizationId instanceof Response) {
    return organizationId;
  }

  const id = c.req.param("id");
  const row = await ownedServer(id, organizationId);
  if (!row) return c.json({ error: "Server not found" }, 404);
  if (row.connectionMode === "outbound") {
    return c.json(
      { error: "Outbound servers connect from the server and do not use SSH provisioning" },
      400,
    );
  }

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
  const organizationId = await resolveActiveWorkspace(c.req.raw);

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

  await db
    .update(domain)
    .set({ status: "pending", statusMessage: null, updatedAt: new Date() })
    .where(eq(domain.serverId, row.id));

  const sync = await enqueueOrRunDomainSync(row.id);
  if (!sync.ok) {
    return c.json({ error: `Proxy sync failed: ${sync.error}` }, 502);
  }

  return c.body(null, "queued" in sync ? 202 : 200);
});

servers.post("/:id/check-connection", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);

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
  if (!isSshConnection(connection)) {
    return c.json({ error: "Outbound servers do not accept SSH connection checks" }, 400);
  }
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

servers.post("/:id/delete-code", async (c) => {
  const active = await resolveActiveWorkspaceUser(c.req.raw.headers);
  if (active instanceof Response) return active;

  const row = await ownedServer(c.req.param("id"), active.organizationId);
  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }
  if (row.isSystem) {
    return c.json({ error: "System servers cannot be deleted" }, 400);
  }

  const code = deleteCode();
  const identifier = deleteVerificationIdentifier({
    organizationId: active.organizationId,
    serverId: row.id,
    userId: active.userId,
  });
  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.delete(verification).where(eq(verification.identifier, identifier));
    await tx.insert(verification).values({
      id: crypto.randomUUID(),
      identifier,
      value: await hashDeleteCode(code),
      expiresAt: new Date(now.getTime() + SERVER_DELETE_CODE_TTL_MS),
      createdAt: now,
      updatedAt: now,
    });
  });

  await sendServerDeleteCodeEmail({
    email: active.email,
    code,
    serverName: row.name,
  });

  return c.json({ ok: true });
});

servers.delete("/:id", async (c) => {
  const active = await resolveActiveWorkspaceUser(c.req.raw.headers);

  if (active instanceof Response) {
    return active;
  }

  const body = (await c.req.json().catch(() => null)) as { code?: unknown } | null;
  const row = await ownedServer(c.req.param("id"), active.organizationId);
  if (!row) {
    return c.json({ error: "Server not found" }, 404);
  }
  if (row.isSystem) {
    return c.json({ error: "System servers cannot be deleted" }, 400);
  }

  const code = normalizeDeleteCode(body?.code);
  if (!/^\d{6}$/.test(code)) {
    return c.json({ error: "Enter the 6-digit code sent to your email" }, 400);
  }

  const identifier = deleteVerificationIdentifier({
    organizationId: active.organizationId,
    serverId: c.req.param("id"),
    userId: active.userId,
  });
  const [challenge] = await db
    .select()
    .from(verification)
    .where(eq(verification.identifier, identifier))
    .limit(1);

  if (!challenge || challenge.expiresAt <= new Date()) {
    if (challenge) await db.delete(verification).where(eq(verification.id, challenge.id));
    return c.json({ error: "Delete code expired. Send a new code and try again." }, 400);
  }

  if (challenge.value !== (await hashDeleteCode(code))) {
    return c.json({ error: "Delete code is incorrect" }, 400);
  }

  const [deleted] = await db
    .delete(server)
    .where(and(eq(server.id, c.req.param("id")), eq(server.organizationId, active.organizationId)))
    .returning({ id: server.id });

  if (!deleted) {
    return c.json({ error: "Server not found" }, 404);
  }

  await db.delete(verification).where(eq(verification.id, challenge.id));

  return c.body(null, 204);
});
