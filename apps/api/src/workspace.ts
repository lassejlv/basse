import { apiToken, db, member } from "@basse/db";
import type { ApiTokenScope } from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { createHash, timingSafeEqual } from "node:crypto";
import { auth } from "./auth";

const SCOPES: ApiTokenScope[] = ["read", "deployments:write", "write"];

function bearerToken(headers: Headers): string | null {
  const header = headers.get("authorization");
  if (!header?.startsWith("Bearer ")) return null;
  return header.slice("Bearer ".length).trim() || null;
}

function hashToken(token: string): string {
  return createHash("sha256").update(token).digest("hex");
}

function safeEqual(a: string, b: string): boolean {
  const left = Buffer.from(a);
  const right = Buffer.from(b);
  return left.length === right.length && timingSafeEqual(left, right);
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

export function requiredApiTokenScope(request: Request): ApiTokenScope | null {
  if (request.method === "GET" || request.method === "HEAD") return "read";
  const path = new URL(request.url).pathname;
  if (path === "/api/deployments" || path === "/api/deployments/rollback") {
    return "deployments:write";
  }
  return "write";
}

export function apiTokenHasRequiredScope(scopes: ApiTokenScope[], request: Request): boolean {
  const required = requiredApiTokenScope(request);
  if (!required) return true;
  return scopes.includes(required) || scopes.includes("write");
}

async function resolveApiTokenWorkspace(request: Request): Promise<string | Response | null> {
  const token = bearerToken(request.headers);
  if (!token) return null;
  const parts = token.split("_");
  if (parts.length < 3 || parts[0] !== "basse") {
    return new Response("Unauthorized", { status: 401 });
  }

  const id = parts[1] ?? "";
  const tokenHash = hashToken(token);
  const [row] = await db.select().from(apiToken).where(eq(apiToken.id, id)).limit(1);
  if (!row || !safeEqual(row.tokenHash, tokenHash)) {
    return new Response("Unauthorized", { status: 401 });
  }
  if (row.expiresAt && row.expiresAt.getTime() <= Date.now()) {
    return new Response("API token expired", { status: 401 });
  }

  const scopes = parseScopes(row.scopes);
  if (!apiTokenHasRequiredScope(scopes, request)) {
    return new Response("Forbidden", { status: 403 });
  }

  await db.update(apiToken).set({ lastUsedAt: new Date() }).where(eq(apiToken.id, row.id));
  return row.organizationId;
}

/**
 * Resolves the caller's active workspace and confirms membership.
 * Returns the organization id, or a Response to short-circuit the request.
 */
export async function resolveActiveWorkspace(input: Headers | Request): Promise<string | Response> {
  if (input instanceof Request) {
    const tokenWorkspace = await resolveApiTokenWorkspace(input);
    if (tokenWorkspace) return tokenWorkspace;
  }

  const headers = input instanceof Request ? input.headers : input;
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

  return organizationId;
}
