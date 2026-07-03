import { db, invitation, member, user } from "@basse/db";
import type {
  InviteTeamMemberInput,
  TeamInvitation,
  TeamMember,
  TeamOverview,
  UpdateTeamMemberInput,
  WorkspaceRole,
} from "@basse/shared";
import { and, eq, ne } from "drizzle-orm";
import { Hono } from "hono";
import { auth } from "../lib/auth";

const ROLES: WorkspaceRole[] = ["owner", "admin", "member"];

export const team = new Hono();

type MemberRow = typeof member.$inferSelect;
type InvitationRow = typeof invitation.$inferSelect;

function normalizeRole(value: unknown): WorkspaceRole | null {
  return ROLES.includes(value as WorkspaceRole) ? (value as WorkspaceRole) : null;
}

export function canManageTeam(role: string): boolean {
  return role === "owner" || role === "admin";
}

function toTeamMember(row: MemberRow & { name: string; email: string }): TeamMember {
  return {
    id: row.id,
    userId: row.userId,
    organizationId: row.organizationId,
    name: row.name,
    email: row.email,
    role: normalizeRole(row.role) ?? "member",
    createdAt: row.createdAt.toISOString(),
  };
}

function toInvitation(row: InvitationRow): TeamInvitation {
  return {
    id: row.id,
    organizationId: row.organizationId,
    email: row.email,
    role: normalizeRole(row.role) ?? "member",
    status: row.status,
    expiresAt: row.expiresAt.toISOString(),
    createdAt: row.createdAt.toISOString(),
  };
}

async function resolveSessionMember(
  headers: Headers,
): Promise<{ organizationId: string; userId: string; role: string } | { response: Response }> {
  const session = await auth.api.getSession({ headers });
  if (!session) return { response: new Response("Unauthorized", { status: 401 }) };
  const organizationId = session.session.activeOrganizationId;
  if (!organizationId) return { response: new Response("No active workspace", { status: 400 }) };

  const [membership] = await db
    .select({ id: member.id, role: member.role })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.userId, session.user.id)))
    .limit(1);
  if (!membership) return { response: new Response("Forbidden", { status: 403 }) };
  return { organizationId, userId: session.user.id, role: membership.role };
}

async function ownerCount(organizationId: string): Promise<number> {
  const rows = await db
    .select({ id: member.id })
    .from(member)
    .where(and(eq(member.organizationId, organizationId), eq(member.role, "owner")));
  return rows.length;
}

team.get("/", async (c) => {
  const resolved = await resolveSessionMember(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;

  const memberRows = await db
    .select({
      id: member.id,
      userId: member.userId,
      organizationId: member.organizationId,
      role: member.role,
      createdAt: member.createdAt,
      name: user.name,
      email: user.email,
    })
    .from(member)
    .innerJoin(user, eq(member.userId, user.id))
    .where(eq(member.organizationId, resolved.organizationId));

  const invitationRows = await db
    .select()
    .from(invitation)
    .where(
      and(eq(invitation.organizationId, resolved.organizationId), eq(invitation.status, "pending")),
    );

  return c.json({
    members: memberRows.map(toTeamMember),
    invitations: invitationRows.map(toInvitation),
  } satisfies TeamOverview);
});

team.post("/invitations", async (c) => {
  const resolved = await resolveSessionMember(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;
  if (!canManageTeam(resolved.role)) return c.json({ error: "Forbidden" }, 403);

  const body = (await c.req.json().catch(() => null)) as Partial<InviteTeamMemberInput> | null;
  const email = typeof body?.email === "string" ? body.email.trim().toLowerCase() : "";
  const role = normalizeRole(body?.role);
  if (!email || !/^[^@\s]+@[^@\s]+\.[^@\s]+$/.test(email)) {
    return c.json({ error: "Valid email is required" }, 400);
  }
  if (!role) return c.json({ error: "Valid role is required" }, 400);

  const [existingUser] = await db.select().from(user).where(eq(user.email, email)).limit(1);
  if (existingUser) {
    const [existingMember] = await db
      .select()
      .from(member)
      .where(
        and(eq(member.organizationId, resolved.organizationId), eq(member.userId, existingUser.id)),
      )
      .limit(1);
    if (existingMember) return c.json({ error: "User is already a member" }, 409);
    const now = new Date();
    const [created] = await db
      .insert(member)
      .values({
        id: crypto.randomUUID(),
        organizationId: resolved.organizationId,
        userId: existingUser.id,
        role,
        createdAt: now,
      })
      .returning();
    return c.json(
      toTeamMember({ ...created!, name: existingUser.name, email: existingUser.email }),
      201,
    );
  }

  const now = new Date();
  const [created] = await db
    .insert(invitation)
    .values({
      id: crypto.randomUUID(),
      organizationId: resolved.organizationId,
      email,
      role,
      status: "pending",
      expiresAt: new Date(now.getTime() + 7 * 24 * 60 * 60 * 1000),
      inviterId: resolved.userId,
      createdAt: now,
    })
    .returning();
  return c.json(toInvitation(created!), 201);
});

team.patch("/members/:id", async (c) => {
  const resolved = await resolveSessionMember(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;
  if (!canManageTeam(resolved.role)) return c.json({ error: "Forbidden" }, 403);

  const body = (await c.req.json().catch(() => null)) as Partial<UpdateTeamMemberInput> | null;
  const role = normalizeRole(body?.role);
  if (!role) return c.json({ error: "Valid role is required" }, 400);

  const [target] = await db
    .select()
    .from(member)
    .where(
      and(eq(member.id, c.req.param("id")), eq(member.organizationId, resolved.organizationId)),
    )
    .limit(1);
  if (!target) return c.json({ error: "Member not found" }, 404);
  if (
    target.role === "owner" &&
    role !== "owner" &&
    (await ownerCount(resolved.organizationId)) <= 1
  ) {
    return c.json({ error: "Cannot demote the last owner" }, 400);
  }

  const [updated] = await db
    .update(member)
    .set({ role })
    .where(eq(member.id, target.id))
    .returning();
  const [userRow] = await db.select().from(user).where(eq(user.id, target.userId)).limit(1);
  return c.json(
    toTeamMember({ ...updated!, name: userRow?.name ?? "", email: userRow?.email ?? "" }),
  );
});

team.delete("/members/:id", async (c) => {
  const resolved = await resolveSessionMember(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;
  if (!canManageTeam(resolved.role)) return c.json({ error: "Forbidden" }, 403);

  const [target] = await db
    .select()
    .from(member)
    .where(
      and(eq(member.id, c.req.param("id")), eq(member.organizationId, resolved.organizationId)),
    )
    .limit(1);
  if (!target) return c.json({ error: "Member not found" }, 404);
  if (target.role === "owner" && (await ownerCount(resolved.organizationId)) <= 1) {
    return c.json({ error: "Cannot remove the last owner" }, 400);
  }
  await db.delete(member).where(and(eq(member.id, target.id), ne(member.userId, resolved.userId)));
  return c.body(null, 204);
});

team.delete("/invitations/:id", async (c) => {
  const resolved = await resolveSessionMember(c.req.raw.headers);
  if ("response" in resolved) return resolved.response;
  if (!canManageTeam(resolved.role)) return c.json({ error: "Forbidden" }, 403);
  const deleted = await db
    .delete(invitation)
    .where(
      and(
        eq(invitation.id, c.req.param("id")),
        eq(invitation.organizationId, resolved.organizationId),
      ),
    )
    .returning({ id: invitation.id });
  if (deleted.length === 0) return c.json({ error: "Invitation not found" }, 404);
  return c.body(null, 204);
});
