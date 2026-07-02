import { db, workspaceSettings } from "@basse/db";
import type { UpdateWorkspaceSettingsInput, WorkspaceSettings } from "@basse/shared";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import { resolveActiveWorkspace } from "./workspace";

type WorkspaceSettingsRow = typeof workspaceSettings.$inferSelect;

const DEFAULT_IMAGE_RETENTION_DAYS = 30;
const MIN_IMAGE_RETENTION_DAYS = 1;
const MAX_IMAGE_RETENTION_DAYS = 365;

function toWorkspaceSettings(row: WorkspaceSettingsRow): WorkspaceSettings {
  return {
    organizationId: row.organizationId,
    imageRetentionDays: row.imageRetentionDays,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

async function getOrCreateWorkspaceSettings(organizationId: string): Promise<WorkspaceSettingsRow> {
  const [existing] = await db
    .select()
    .from(workspaceSettings)
    .where(eq(workspaceSettings.organizationId, organizationId))
    .limit(1);
  if (existing) return existing;

  const now = new Date();
  const [created] = await db
    .insert(workspaceSettings)
    .values({
      organizationId,
      imageRetentionDays: DEFAULT_IMAGE_RETENTION_DAYS,
      createdAt: now,
      updatedAt: now,
    })
    .returning();

  if (!created) throw new Error("Failed to create workspace settings");
  return created;
}

export const workspaceSettingsRoutes = new Hono();

workspaceSettingsRoutes.get("/settings", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const row = await getOrCreateWorkspaceSettings(organizationId);
  return c.json(toWorkspaceSettings(row));
});

workspaceSettingsRoutes.put("/settings", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as UpdateWorkspaceSettingsInput | null;
  const imageRetentionDays = Number(body?.imageRetentionDays);

  if (
    !Number.isInteger(imageRetentionDays) ||
    imageRetentionDays < MIN_IMAGE_RETENTION_DAYS ||
    imageRetentionDays > MAX_IMAGE_RETENTION_DAYS
  ) {
    return c.json(
      {
        error: `imageRetentionDays must be an integer from ${MIN_IMAGE_RETENTION_DAYS} to ${MAX_IMAGE_RETENTION_DAYS}`,
      },
      400,
    );
  }

  await getOrCreateWorkspaceSettings(organizationId);

  const [updated] = await db
    .update(workspaceSettings)
    .set({ imageRetentionDays, updatedAt: new Date() })
    .where(eq(workspaceSettings.organizationId, organizationId))
    .returning();

  if (!updated) return c.json({ error: "Workspace settings not found" }, 404);
  return c.json(toWorkspaceSettings(updated));
});
