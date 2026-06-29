import { db, member } from "@basse/db";
import { and, eq } from "drizzle-orm";
import { auth } from "./auth";

/**
 * Resolves the caller's active workspace and confirms membership.
 * Returns the organization id, or a Response to short-circuit the request.
 */
export async function resolveActiveWorkspace(headers: Headers): Promise<string | Response> {
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
