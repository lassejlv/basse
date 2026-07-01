import { app, db, s3Connection } from "@basse/db";
import type {
  CreateS3ConnectionInput,
  S3Connection as S3ConnectionDto,
  UpdateS3ConnectionInput,
} from "@basse/shared";
import { S3Client } from "bun";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { decryptSecret, encryptSecret } from "./crypto";
import { resolveActiveWorkspace } from "./workspace";

export type S3ConnectionRow = typeof s3Connection.$inferSelect;

const TEST_OBJECT_KEY = "basse/.connection-test";

/** Builds a Bun S3 client for a stored connection (decrypts the secret). */
export async function s3ClientForConnection(row: S3ConnectionRow): Promise<S3Client> {
  return new S3Client({
    accessKeyId: row.accessKeyId,
    secretAccessKey: await decryptSecret(row.secretAccessKey),
    bucket: row.bucket,
    ...(row.endpoint ? { endpoint: row.endpoint } : {}),
    ...(row.region ? { region: row.region } : {}),
  });
}

export async function ownedS3Connection(
  id: string,
  organizationId: string,
): Promise<S3ConnectionRow | null> {
  const [row] = await db
    .select()
    .from(s3Connection)
    .where(and(eq(s3Connection.id, id), eq(s3Connection.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
}

/** Round-trips a tiny object to prove the credentials and bucket work. */
async function testConnection(row: S3ConnectionRow): Promise<{ ok: boolean; message: string }> {
  try {
    const client = await s3ClientForConnection(row);
    const file = client.file(TEST_OBJECT_KEY);
    await file.write("basse connection test");
    await file.delete();
    return { ok: true, message: "" };
  } catch (error) {
    return { ok: false, message: error instanceof Error ? error.message : "Connection failed" };
  }
}

function toDto(row: S3ConnectionRow): S3ConnectionDto {
  return {
    id: row.id,
    name: row.name,
    endpoint: row.endpoint,
    region: row.region,
    bucket: row.bucket,
    accessKeyId: row.accessKeyId,
    secretHint: row.secretHint,
    status: row.status,
    statusMessage: row.statusMessage,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

function secretHint(secret: string): string {
  return secret.length > 4 ? `…${secret.slice(-4)}` : "…";
}

function invalidInput(body: Partial<CreateS3ConnectionInput>, requireAll: boolean): string | null {
  const check = (key: keyof CreateS3ConnectionInput, label: string) => {
    const value = body[key];
    if (typeof value === "undefined") return requireAll ? `${label} is required` : null;
    if (typeof value !== "string") return `${label} must be a string`;
    if (["name", "bucket", "accessKeyId", "secretAccessKey"].includes(key) && !value.trim()) {
      return `${label} is required`;
    }
    if (value.length > 500) return `${label} is too long`;
    return null;
  };
  for (const [key, label] of [
    ["name", "Name"],
    ["bucket", "Bucket"],
    ["accessKeyId", "Access key id"],
    ["secretAccessKey", "Secret access key"],
  ] as const) {
    const error = check(key, label);
    if (error) return error;
  }
  for (const key of ["endpoint", "region"] as const) {
    const value = body[key];
    if (typeof value !== "undefined" && typeof value !== "string") return `Invalid ${key}`;
    if (typeof value === "string" && value.length > 500) return `${key} is too long`;
  }
  if (typeof body.endpoint === "string" && body.endpoint.trim()) {
    try {
      const url = new URL(body.endpoint.trim());
      if (url.protocol !== "https:" && url.protocol !== "http:") return "Invalid endpoint URL";
    } catch {
      return "Invalid endpoint URL";
    }
  }
  return null;
}

export const s3 = new Hono();

s3.get("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const rows = await db
    .select()
    .from(s3Connection)
    .where(eq(s3Connection.organizationId, organizationId));
  return c.json(rows.map(toDto));
});

s3.post("/", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as CreateS3ConnectionInput | null;
  if (!body) return c.json({ error: "Invalid request body" }, 400);
  const invalid = invalidInput(body, true);
  if (invalid) return c.json({ error: invalid }, 400);

  const now = new Date();
  const row: S3ConnectionRow = {
    id: crypto.randomUUID(),
    organizationId,
    name: body.name.trim(),
    endpoint: body.endpoint?.trim() || null,
    region: body.region?.trim() || null,
    bucket: body.bucket.trim(),
    accessKeyId: body.accessKeyId.trim(),
    secretAccessKey: await encryptSecret(body.secretAccessKey),
    secretHint: secretHint(body.secretAccessKey),
    status: "active",
    statusMessage: null,
    createdAt: now,
    updatedAt: now,
  };

  const test = await testConnection(row);
  row.status = test.ok ? "active" : "error";
  row.statusMessage = test.ok ? null : test.message;

  const [created] = await db.insert(s3Connection).values(row).returning();
  if (!created) return c.json({ error: "Failed to create connection" }, 500);
  return c.json(toDto(created), 201);
});

s3.patch("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedS3Connection(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "Connection not found" }, 404);

  const body = (await c.req.json().catch(() => null)) as UpdateS3ConnectionInput | null;
  if (!body) return c.json({ error: "Invalid request body" }, 400);
  const invalid = invalidInput(body, false);
  if (invalid) return c.json({ error: invalid }, 400);

  const updated: S3ConnectionRow = {
    ...existing,
    name: typeof body.name === "string" ? body.name.trim() : existing.name,
    endpoint: typeof body.endpoint === "string" ? body.endpoint.trim() || null : existing.endpoint,
    region: typeof body.region === "string" ? body.region.trim() || null : existing.region,
    bucket: typeof body.bucket === "string" ? body.bucket.trim() : existing.bucket,
    accessKeyId:
      typeof body.accessKeyId === "string" ? body.accessKeyId.trim() : existing.accessKeyId,
    secretAccessKey:
      typeof body.secretAccessKey === "string"
        ? await encryptSecret(body.secretAccessKey)
        : existing.secretAccessKey,
    secretHint:
      typeof body.secretAccessKey === "string"
        ? secretHint(body.secretAccessKey)
        : existing.secretHint,
    updatedAt: new Date(),
  };

  const test = await testConnection(updated);
  updated.status = test.ok ? "active" : "error";
  updated.statusMessage = test.ok ? null : test.message;

  const [saved] = await db
    .update(s3Connection)
    .set({
      name: updated.name,
      endpoint: updated.endpoint,
      region: updated.region,
      bucket: updated.bucket,
      accessKeyId: updated.accessKeyId,
      secretAccessKey: updated.secretAccessKey,
      secretHint: updated.secretHint,
      status: updated.status,
      statusMessage: updated.statusMessage,
      updatedAt: updated.updatedAt,
    })
    .where(eq(s3Connection.id, existing.id))
    .returning();
  return c.json(toDto(saved ?? updated));
});

s3.post("/:id/test", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedS3Connection(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "Connection not found" }, 404);

  const test = await testConnection(existing);
  const [saved] = await db
    .update(s3Connection)
    .set({
      status: test.ok ? "active" : "error",
      statusMessage: test.ok ? null : test.message,
      updatedAt: new Date(),
    })
    .where(eq(s3Connection.id, existing.id))
    .returning();
  return c.json(toDto(saved ?? existing));
});

s3.delete("/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const existing = await ownedS3Connection(c.req.param("id"), organizationId);
  if (!existing) return c.json({ error: "Connection not found" }, 404);

  // Detach apps that pointed backups at this connection, then drop the row
  // (database_backup.s3_connection_id nulls out via FK).
  await db
    .update(app)
    .set({ backupS3ConnectionId: null, updatedAt: new Date() })
    .where(eq(app.backupS3ConnectionId, existing.id));
  await db.delete(s3Connection).where(eq(s3Connection.id, existing.id));
  return c.json({ ok: true });
});
