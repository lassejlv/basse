import { and, eq } from "drizzle-orm";
import { db } from "./client";
import { member, organization } from "./schema";

export type PersonalWorkspaceUser = {
  id: string;
  name: string;
  email: string;
};

export async function ensurePersonalWorkspace(user: PersonalWorkspaceUser) {
  const organizationId = crypto.randomUUID();
  const now = new Date();
  const name = personalWorkspaceName(user);
  const slug = `personal-${user.id}`;

  const [existingMembership] = await db
    .select({ organizationId: member.organizationId })
    .from(member)
    .where(and(eq(member.userId, user.id), eq(member.role, "owner")))
    .limit(1);

  if (existingMembership) {
    return;
  }

  await db
    .insert(organization)
    .values({
      id: organizationId,
      name,
      slug,
      logo: null,
      createdAt: now,
      metadata: JSON.stringify({ type: "personal" }),
    })
    .onConflictDoNothing();

  const [workspace] = await db
    .select({ id: organization.id })
    .from(organization)
    .where(eq(organization.slug, slug))
    .limit(1);

  if (!workspace) {
    return;
  }

  await db
    .insert(member)
    .values({
      id: crypto.randomUUID(),
      organizationId: workspace.id,
      userId: user.id,
      role: "owner",
      createdAt: now,
    })
    .onConflictDoNothing();
}

function personalWorkspaceName(user: PersonalWorkspaceUser) {
  const name = user.name.trim();

  if (name) {
    return `${name}'s workspace`;
  }

  return `${user.email}'s workspace`;
}
