import {
  app,
  db,
  envVar,
  environment,
  environmentEnvVar,
  project,
  projectEnvVar,
} from "@basse/db";
import { and, eq } from "drizzle-orm";
import { decryptSecret } from "./crypto";

type AppRow = typeof app.$inferSelect;

const ENV_REFERENCE_PATTERN = /\{\{\s*(shared|env)\.([A-Za-z_][A-Za-z0-9_]*)\s*\}\}/g;

export async function loadResolvedEnvMap(appRow: AppRow): Promise<Record<string, string>> {
  const [context] = await db
    .select({ projectId: project.id, environmentId: environment.id })
    .from(environment)
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(environment.id, appRow.environmentId))
    .limit(1);

  if (!context) {
    throw new Error("App environment was not found");
  }

  const [appRows, projectRows, environmentRows] = await Promise.all([
    db.select().from(envVar).where(eq(envVar.appId, appRow.id)),
    db.select().from(projectEnvVar).where(eq(projectEnvVar.projectId, context.projectId)),
    db
      .select()
      .from(environmentEnvVar)
      .where(eq(environmentEnvVar.environmentId, context.environmentId)),
  ]);

  const shared = new Map<string, string>();
  const env = new Map<string, string>();

  for (const row of projectRows) shared.set(row.key, await decryptSecret(row.value));
  for (const row of environmentRows) env.set(row.key, await decryptSecret(row.value));

  const envMap: Record<string, string> = {};
  for (const row of appRows) {
    const value = await decryptSecret(row.value);
    envMap[row.key] = resolveReferences(value, { shared, env }, row.key);
  }

  return envMap;
}

function resolveReferences(
  value: string,
  refs: { shared: Map<string, string>; env: Map<string, string> },
  appKey: string,
): string {
  return value.replace(
    ENV_REFERENCE_PATTERN,
    (_match, scope: "shared" | "env", key: string) => {
      const resolved = refs[scope].get(key);
      if (resolved === undefined) {
        throw new Error(`Missing ${scope} env reference ${key} used by ${appKey}`);
      }
      return resolved;
    },
  );
}

export async function ownedProjectId(projectId: string, organizationId: string): Promise<boolean> {
  const [row] = await db
    .select({ id: project.id })
    .from(project)
    .where(and(eq(project.id, projectId), eq(project.organizationId, organizationId)))
    .limit(1);
  return Boolean(row);
}

export async function ownedEnvironmentId(
  environmentId: string,
  organizationId: string,
): Promise<{ id: string; projectId: string; name: string } | null> {
  const [row] = await db
    .select({ id: environment.id, projectId: project.id, name: environment.name })
    .from(environment)
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(and(eq(environment.id, environmentId), eq(project.organizationId, organizationId)))
    .limit(1);
  return row ?? null;
}
