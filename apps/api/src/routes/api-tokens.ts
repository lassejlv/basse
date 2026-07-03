import { apiToken, db, member } from "@basse/db";
import type {
  ApiToken,
  ApiTokenScope,
  CreateApiTokenInput,
  CreateApiTokenResult,
} from "@basse/shared";
import { and, desc, eq } from "drizzle-orm";
import { Hono } from "hono";
import { createHash, randomBytes } from "node:crypto";
import { auth } from "../lib/auth";

const SCOPES: ApiTokenScope[] = ["read", "deployments:write", "write"];

type ApiTokenRow = typeof apiToken.$inferSelect;

export const apiTokens = new Hono();

function toApiToken(row: ApiTokenRow): ApiToken {
  return {
    id: row.id,
    organizationId: row.organizationId,
    name: row.name,
    tokenPrefix: row.tokenPrefix,
    scopes: parseScopes(row.scopes),
    lastUsedAt: row.lastUsedAt?.toISOString() ?? null,
    expiresAt: row.expiresAt?.toISOString() ?? null,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

function parseScopes(value: string): ApiTokenScope[] {
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed.filter((scope): scope is ApiTokenScope =>
      SCOPES.includes(scope as ApiTokenScope),
    );
  } catch {
    return [];
  }
}

function normalizeScopes(value: unknown): ApiTokenScope[] | null {
  if (!Array.isArray(value)) return null;
  const scopes = [...new Set(value)].filter((scope): scope is ApiTokenScope =>
    SCOPES.includes(scope as ApiTokenScope),
  );
  return scopes.length > 0 ? scopes : null;
}

function hashToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

async function resolveSessionWorkspace(
  headers: Headers,
): Promise<{ organizationId: string; userId: string } | { response: Response }> {
  const session = await auth.api.getSession({ headers });
  if (!session) return { response: new Response("Unauthorized", { status: 401 }) };

  const organizationId = session.session.activeOrganizationId;
  if (!organizationId) return { response: new Response("No active workspace", { status: 400 }) };

  const [membership] = await db
    .select({ id: member.id })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.userId, session.user.id)))
    .limit(1);
  if (!membership) return { response: new Response("Forbidden", { status: 403 }) };

  return { organizationId, userId: session.user.id };
}

apiTokens.get("/", async (c) => {
  const resolved = await resolveSessionWorkspace(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;

  const rows = await db
    .select()
    .from(apiToken)
    .where(eq(apiToken.organizationId, resolved.organizationId))
    .orderBy(desc(apiToken.createdAt));

  return c.json(rows.map(toApiToken));
});

apiTokens.post("/", async (c) => {
  const resolved = await resolveSessionWorkspace(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;

  const body = (await c.req.json().catch(() => null)) as Partial<CreateApiTokenInput> | null;
  const name = typeof body?.name === "string" ? body.name.trim() : "";
  if (!name) return c.json({ error: "name is required" }, 400);
  if (name.length > 80) return c.json({ error: "name is too long" }, 400);

  const scopes = normalizeScopes(body?.scopes);
  if (!scopes) return c.json({ error: "Choose at least one valid scope" }, 400);

  let expiresAt: Date | null = null;
  if (body && "expiresAt" in body && body.expiresAt) {
    expiresAt = new Date(body.expiresAt);
    if (Number.isNaN(expiresAt.getTime())) {
      return c.json({ error: "expiresAt must be an ISO timestamp" }, 400);
    }
    if (expiresAt.getTime() <= Date.now()) {
      return c.json({ error: "expiresAt must be in the future" }, 400);
    }
  }

  const id = crypto.randomUUID();
  const secret = randomBytes(32).toString("base64url");
  const token = `basse_${id}_${secret}`;
  const now = new Date();
  const [created] = await db
    .insert(apiToken)
    .values({
      id,
      organizationId: resolved.organizationId,
      createdByUserId: resolved.userId,
      name,
      tokenHash: hashToken(token),
      tokenPrefix: `basse_${id.slice(0, 8)}`,
      scopes: JSON.stringify(scopes),
      expiresAt,
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) return c.json({ error: "Failed to create API token" }, 500);
  return c.json({ token, apiToken: toApiToken(created) } satisfies CreateApiTokenResult, 201);
});

apiTokens.delete("/:id", async (c) => {
  const resolved = await resolveSessionWorkspace(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;

  const id = c.req.param("id");
  const deleted = await db
    .delete(apiToken)
    .where(and(eq(apiToken.id, id), eq(apiToken.organizationId, resolved.organizationId)))
    .returning({ id: apiToken.id });
  if (deleted.length === 0) return c.json({ error: "API token not found" }, 404);
  return c.body(null, 204);
});
