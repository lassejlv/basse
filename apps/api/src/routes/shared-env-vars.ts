import { app, db, environment, environmentEnvVar, project, projectEnvVar } from "@basse/db";
import type {
  EnvReferenceSuggestion,
  SetSharedEnvVarsInput,
  SharedEnvVarMasked,
  SharedEnvVarPlain,
} from "@basse/shared";
import { and, eq } from "drizzle-orm";
import { Hono } from "hono";
import { ownedApp } from "./apps";
import { decryptSecret, encryptSecret } from "../lib/crypto";
import { ownedEnvironmentId, ownedProjectId } from "../deploy/env-resolver";
import { resolveActiveWorkspace } from "../lib/workspace";

type SharedRow = {
  key: string;
  value: string;
  updatedAt: Date;
};

const ENV_KEY_PATTERN = /^[A-Za-z_][A-Za-z0-9_]*$/;

export const projectSharedEnvVars = new Hono();
export const environmentSharedEnvVars = new Hono();
export const appEnvReferences = new Hono();

async function maskedValue(encrypted: string): Promise<string> {
  try {
    const value = await decryptSecret(encrypted);
    return value.length <= 4 ? "••••" : `••••${value.slice(-4)}`;
  } catch {
    return "••••";
  }
}

async function toMasked(row: SharedRow): Promise<SharedEnvVarMasked> {
  return {
    key: row.key,
    valueHint: await maskedValue(row.value),
    updatedAt: row.updatedAt.toISOString(),
  };
}

async function toPlain(row: SharedRow): Promise<SharedEnvVarPlain> {
  return {
    key: row.key,
    value: await decryptSecret(row.value),
  };
}

function parseVars(body: Partial<SetSharedEnvVarsInput> | null) {
  const vars = Array.isArray(body?.vars) ? body.vars : null;
  if (!vars) return { ok: false as const, error: "vars must be an array" };

  const cleaned = new Map<string, string>();
  for (const v of vars) {
    const key = typeof v?.key === "string" ? v.key.trim() : "";
    const value = typeof v?.value === "string" ? v.value : "";
    if (!key) continue;
    if (!ENV_KEY_PATTERN.test(key)) {
      return { ok: false as const, error: `invalid variable name: ${key}` };
    }
    cleaned.set(key, value);
  }

  return {
    ok: true as const,
    vars: [...cleaned].map(([key, value]) => ({ key, value })),
  };
}

projectSharedEnvVars.get("/:id/shared-env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProjectId(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const rows = await db
    .select()
    .from(projectEnvVar)
    .where(eq(projectEnvVar.projectId, projectId))
    .orderBy(projectEnvVar.key);
  return c.json(await Promise.all(rows.map(toMasked)));
});

projectSharedEnvVars.get("/:id/shared-env-vars/reveal", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProjectId(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const rows = await db
    .select()
    .from(projectEnvVar)
    .where(eq(projectEnvVar.projectId, projectId))
    .orderBy(projectEnvVar.key);
  return c.json(await Promise.all(rows.map(toPlain)));
});

projectSharedEnvVars.put("/:id/shared-env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const projectId = c.req.param("id");
  if (!(await ownedProjectId(projectId, organizationId))) {
    return c.json({ error: "Project not found" }, 404);
  }

  const parsed = parseVars(
    (await c.req.json().catch(() => null)) as Partial<SetSharedEnvVarsInput> | null,
  );
  if (!parsed.ok) return c.json({ error: parsed.error }, 400);

  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.delete(projectEnvVar).where(eq(projectEnvVar.projectId, projectId));
    if (parsed.vars.length > 0) {
      await tx.insert(projectEnvVar).values(
        await Promise.all(
          parsed.vars.map(async (v) => ({
            id: crypto.randomUUID(),
            projectId,
            key: v.key,
            value: await encryptSecret(v.value),
            createdAt: now,
            updatedAt: now,
          })),
        ),
      );
    }
  });

  return c.body(null, 204);
});

environmentSharedEnvVars.get("/:id/shared-env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedEnvironmentId(c.req.param("id"), organizationId);
  if (!owned) return c.json({ error: "Environment not found" }, 404);

  const rows = await db
    .select()
    .from(environmentEnvVar)
    .where(eq(environmentEnvVar.environmentId, owned.id))
    .orderBy(environmentEnvVar.key);
  return c.json(await Promise.all(rows.map(toMasked)));
});

environmentSharedEnvVars.get("/:id/shared-env-vars/reveal", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedEnvironmentId(c.req.param("id"), organizationId);
  if (!owned) return c.json({ error: "Environment not found" }, 404);

  const rows = await db
    .select()
    .from(environmentEnvVar)
    .where(eq(environmentEnvVar.environmentId, owned.id))
    .orderBy(environmentEnvVar.key);
  return c.json(await Promise.all(rows.map(toPlain)));
});

environmentSharedEnvVars.put("/:id/shared-env-vars", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedEnvironmentId(c.req.param("id"), organizationId);
  if (!owned) return c.json({ error: "Environment not found" }, 404);

  const parsed = parseVars(
    (await c.req.json().catch(() => null)) as Partial<SetSharedEnvVarsInput> | null,
  );
  if (!parsed.ok) return c.json({ error: parsed.error }, 400);

  const now = new Date();
  await db.transaction(async (tx) => {
    await tx.delete(environmentEnvVar).where(eq(environmentEnvVar.environmentId, owned.id));
    if (parsed.vars.length > 0) {
      await tx.insert(environmentEnvVar).values(
        await Promise.all(
          parsed.vars.map(async (v) => ({
            id: crypto.randomUUID(),
            environmentId: owned.id,
            key: v.key,
            value: await encryptSecret(v.value),
            createdAt: now,
            updatedAt: now,
          })),
        ),
      );
    }
  });

  return c.body(null, 204);
});

appEnvReferences.get("/:appId/env-references", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const owned = await ownedApp(c.req.param("appId"), organizationId);
  if (!owned) return c.json({ error: "App not found" }, 404);

  const [context] = await db
    .select({
      projectId: project.id,
      environmentId: environment.id,
      environmentName: environment.name,
    })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(app.id, owned.id), eq(project.organizationId, organizationId)))
    .limit(1);
  if (!context) return c.json({ error: "App not found" }, 404);

  const [projectRows, environmentRows] = await Promise.all([
    db.select().from(projectEnvVar).where(eq(projectEnvVar.projectId, context.projectId)),
    db
      .select()
      .from(environmentEnvVar)
      .where(eq(environmentEnvVar.environmentId, context.environmentId)),
  ]);

  const shared: EnvReferenceSuggestion[] = await Promise.all(
    projectRows.map(async (row) => ({
      scope: "shared",
      key: row.key,
      insertText: `{{shared.${row.key}}}`,
      label: `shared.${row.key}`,
      valueHint: await maskedValue(row.value),
    })),
  );
  const env: EnvReferenceSuggestion[] = await Promise.all(
    environmentRows.map(async (row) => ({
      scope: "env",
      key: row.key,
      insertText: `{{env.${row.key}}}`,
      label: `env.${row.key}`,
      valueHint: await maskedValue(row.value),
      environmentId: context.environmentId,
      environmentName: context.environmentName,
    })),
  );

  return c.json([...shared, ...env].sort((a, b) => a.label.localeCompare(b.label)));
});
