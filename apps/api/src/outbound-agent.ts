import { agentCommand, db, server } from "@basse/db";
import { and, asc, eq, or, sql } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret } from "./crypto";

type AgentCommandRow = typeof agentCommand.$inferSelect;

export type OutboundAgentRequest = {
  serverId: string;
  method: string;
  path: string;
  body?: unknown;
  timeoutMs: number;
};

export type OutboundAgentResponse = {
  status: number;
  body: string;
};

const POLL_LEASE_MS = 60_000;
const POLL_WAIT_MS = 1000;

export const outboundAgent = new Hono();

export async function agentTokenHash(token: string): Promise<string> {
  const digest = await crypto.subtle.digest("SHA-256", new TextEncoder().encode(token));
  return Buffer.from(digest).toString("hex");
}

export async function sendOutboundAgentRequest(
  input: OutboundAgentRequest,
): Promise<OutboundAgentResponse> {
  const now = new Date();
  const [created] = await db
    .insert(agentCommand)
    .values({
      id: crypto.randomUUID(),
      serverId: input.serverId,
      method: input.method,
      path: input.path,
      body: typeof input.body === "undefined" ? null : JSON.stringify(input.body),
      status: "queued",
      createdAt: now,
      updatedAt: now,
    })
    .returning();
  if (!created) throw new Error("Could not queue outbound agent command");

  const deadline = Date.now() + input.timeoutMs;
  while (Date.now() < deadline) {
    const [row] = await db
      .select()
      .from(agentCommand)
      .where(eq(agentCommand.id, created.id))
      .limit(1);
    if (!row) throw new Error("Outbound agent command disappeared");
    if (row.status === "completed") {
      return { status: row.responseStatus ?? 200, body: row.responseBody ?? "" };
    }
    if (row.status === "failed" || row.status === "expired") {
      throw new Error(row.error ?? `Outbound agent command ${row.status}`);
    }
    await Bun.sleep(Math.min(POLL_WAIT_MS, Math.max(0, deadline - Date.now())));
  }

  await db
    .update(agentCommand)
    .set({ status: "expired", error: "Command timed out", updatedAt: new Date() })
    .where(eq(agentCommand.id, created.id));
  throw new Error("Outbound agent command timed out");
}

outboundAgent.post("/poll", async (c) => {
  try {
    const auth = c.req.header("authorization") ?? "";
    const token = auth.startsWith("Bearer ") ? auth.slice("Bearer ".length).trim() : "";
    if (!token) return c.json({ error: "unauthorized" }, 401);

    const tokenHash = await agentTokenHash(token);
    const [row] = await db
      .select()
      .from(server)
      .where(and(eq(server.agentTokenHash, tokenHash), eq(server.connectionMode, "outbound")))
      .limit(1);
    if (!row?.agentToken) return c.json({ error: "unauthorized" }, 401);

    const storedToken = await decryptSecret(row.agentToken).catch(() => null);
    if (storedToken !== token) return c.json({ error: "unauthorized" }, 401);

    const now = new Date();
    await db
      .update(server)
      .set({ status: "active", statusMessage: null, lastSeenAt: now, updatedAt: now })
      .where(eq(server.id, row.id));

    const command = await claimNextCommand(row.id, now);
    if (!command) return c.body(null, 204);

    return c.json({
      id: command.id,
      method: command.method,
      path: command.path,
      body: command.body ? JSON.parse(command.body) : null,
    });
  } catch (error) {
    console.error("[outbound-agent] poll failed", error);
    return c.json({ error: "poll failed" }, 500);
  }
});

outboundAgent.post("/commands/:id/result", async (c) => {
  try {
    const auth = c.req.header("authorization") ?? "";
    const token = auth.startsWith("Bearer ") ? auth.slice("Bearer ".length).trim() : "";
    if (!token) return c.json({ error: "unauthorized" }, 401);

    const tokenHash = await agentTokenHash(token);
    const [serverRow] = await db
      .select({ id: server.id, agentToken: server.agentToken })
      .from(server)
      .where(and(eq(server.agentTokenHash, tokenHash), eq(server.connectionMode, "outbound")))
      .limit(1);
    if (!serverRow?.agentToken) return c.json({ error: "unauthorized" }, 401);

    const storedToken = await decryptSecret(serverRow.agentToken).catch(() => null);
    if (storedToken !== token) return c.json({ error: "unauthorized" }, 401);

    const body = (await c.req.json().catch(() => null)) as {
      status?: unknown;
      body?: unknown;
      error?: unknown;
    } | null;
    const responseStatus = typeof body?.status === "number" ? body.status : 502;
    const responseBody = typeof body?.body === "string" ? body.body : "";
    const error = typeof body?.error === "string" ? body.error : null;
    const now = new Date();

    await db
      .update(agentCommand)
      .set({
        status: error ? "failed" : "completed",
        responseStatus,
        responseBody,
        error,
        completedAt: now,
        updatedAt: now,
      })
      .where(and(eq(agentCommand.id, c.req.param("id")), eq(agentCommand.serverId, serverRow.id)));

    return c.json({ ok: true });
  } catch (error) {
    console.error("[outbound-agent] result failed", error);
    return c.json({ error: "result failed" }, 500);
  }
});

async function claimNextCommand(serverId: string, now: Date): Promise<AgentCommandRow | null> {
  const [command] = await db
    .select()
    .from(agentCommand)
    .where(
      and(
        eq(agentCommand.serverId, serverId),
        or(
          eq(agentCommand.status, "queued"),
          and(eq(agentCommand.status, "running"), sql`${agentCommand.leaseUntil} < ${now}`),
        ),
      ),
    )
    .orderBy(asc(agentCommand.createdAt))
    .limit(1);
  if (!command) return null;

  const leaseUntil = new Date(now.getTime() + POLL_LEASE_MS);
  const [claimed] = await db
    .update(agentCommand)
    .set({ status: "running", leaseUntil, updatedAt: now })
    .where(eq(agentCommand.id, command.id))
    .returning();
  return claimed ?? null;
}
