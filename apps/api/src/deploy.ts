import {
  app,
  appServer,
  db,
  deployment,
  depotConnection,
  environment,
  envVar,
  project,
  server,
} from "@basse/db";
import { and, eq, inArray, ne } from "drizzle-orm";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { deployApp, ensureProxy } from "./agent-client";
import {
  buildImage,
  buildImageOnServer,
  cloneRepo,
  mintPullToken,
  resolveBuildKind,
} from "./builder";
import { decryptSecret } from "./crypto";
import { connectionFromServer } from "./server-connection";
import { runScript } from "./ssh";

type DeploymentRow = typeof deployment.$inferSelect;
type AppVolume = { hostPath: string; containerPath: string; readOnly: boolean };

const NON_TERMINAL: DeploymentRow["status"][] = ["queued", "building", "deploying"];
const DEFAULT_POSTGRES_VERSION = "18";
const POSTGRES_PORT = 5432;

async function setStatus(
  id: string,
  status: DeploymentRow["status"],
  extra: Partial<Pick<DeploymentRow, "imageRef" | "buildId" | "commitSha">> = {},
): Promise<void> {
  await db
    .update(deployment)
    .set({ status, updatedAt: new Date(), ...extra })
    .where(eq(deployment.id, id));
}

/** Debounced append into deployment.logs so a chatty build doesn't hammer the DB. */
function makeLogger(deploymentId: string) {
  let pending = "";
  let timer: ReturnType<typeof setTimeout> | undefined;

  const flush = async () => {
    if (!pending) return;
    const chunk = pending;
    pending = "";
    const [row] = await db
      .select({ logs: deployment.logs })
      .from(deployment)
      .where(eq(deployment.id, deploymentId))
      .limit(1);
    await db
      .update(deployment)
      .set({ logs: (row?.logs ?? "") + chunk, updatedAt: new Date() })
      .where(eq(deployment.id, deploymentId));
  };

  return {
    line(text: string) {
      pending += `${text}\n`;
      if (!timer) {
        timer = setTimeout(() => {
          timer = undefined;
          void flush();
        }, 1000);
      }
    },
    async done() {
      if (timer) clearTimeout(timer);
      await flush();
    },
  };
}

function parseVolumes(value: string): AppVolume[] {
  try {
    const parsed = JSON.parse(value) as unknown;
    if (!Array.isArray(parsed)) return [];
    return parsed
      .map((item) => {
        if (!item || typeof item !== "object") return null;
        const volume = item as Partial<AppVolume>;
        if (typeof volume.hostPath !== "string" || typeof volume.containerPath !== "string") {
          return null;
        }
        return {
          hostPath: volume.hostPath,
          containerPath: volume.containerPath,
          readOnly: volume.readOnly === true,
        };
      })
      .filter((volume): volume is AppVolume => Boolean(volume));
  } catch {
    return [];
  }
}

function shellQuote(value: string): string {
  return `'${value.replaceAll("'", "'\\''")}'`;
}

function databaseImage(appRow: typeof app.$inferSelect): string {
  return appRow.databaseKind === "postgres"
    ? `postgres:${appRow.databaseVersion ?? DEFAULT_POSTGRES_VERSION}`
    : "";
}

function databaseVolume(appId: string): AppVolume {
  return {
    hostPath: `basse-postgres-${appId}`,
    containerPath: "/var/lib/postgresql/data",
    readOnly: false,
  };
}

async function resolveDepotRegistryForImage(
  appRow: typeof app.$inferSelect,
  imageRef: string,
): Promise<{ host: string; user: string; token: string } | undefined> {
  const [env] = await db
    .select()
    .from(environment)
    .where(eq(environment.id, appRow.environmentId))
    .limit(1);
  const [proj] = env
    ? await db.select().from(project).where(eq(project.id, env.projectId)).limit(1)
    : [];
  const [depot] = proj
    ? await db
        .select()
        .from(depotConnection)
        .where(eq(depotConnection.organizationId, proj.organizationId))
        .limit(1)
    : [];

  if (!depot || !depot.orgId || !imageRef.startsWith(`${depot.orgId}.registry.depot.dev/`)) {
    return undefined;
  }

  const depotToken = await decryptSecret(depot.token);
  return {
    host: `${depot.orgId}.registry.depot.dev`,
    user: "x-token",
    token: await mintPullToken(depotToken, depot.projectId),
  };
}

/**
 * Builds and deploys a deployment. Idempotent-ish and NEVER throws — every
 * failure lands the deployment in a terminal status with logs. The deployment
 * row is the single UI source of truth.
 */
export async function runDeployment(deploymentId: string): Promise<void> {
  const log = makeLogger(deploymentId);
  let ctxDir: string | null = null;

  try {
    // Claim: queued/failed -> building (the DB row is the lock).
    const claimed = await db
      .update(deployment)
      .set({ status: "building", updatedAt: new Date() })
      .where(and(eq(deployment.id, deploymentId), inArray(deployment.status, ["queued", "failed"])))
      .returning({ id: deployment.id });
    if (!claimed[0]) return;

    const [dep] = await db
      .select()
      .from(deployment)
      .where(eq(deployment.id, deploymentId))
      .limit(1);
    if (!dep) return;

    const [appRow] = await db.select().from(app).where(eq(app.id, dep.appId)).limit(1);
    if (!appRow) {
      log.line("App no longer exists.");
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    // Supersede any other in-flight deploy of the same app.
    await db
      .update(deployment)
      .set({ status: "cancelled", updatedAt: new Date() })
      .where(
        and(
          eq(deployment.appId, appRow.id),
          ne(deployment.id, deploymentId),
          inArray(deployment.status, NON_TERMINAL),
        ),
      );

    const serverRows = await db
      .select({ server })
      .from(appServer)
      .innerJoin(server, eq(appServer.serverId, server.id))
      .where(eq(appServer.appId, appRow.id));
    const targetServers = serverRows.map((row) => row.server);

    if (targetServers.length === 0) {
      log.line("No servers attached to this app.");
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    const inactive = targetServers.find((srv) => srv.status !== "active" || !srv.agentToken);
    if (inactive) {
      log.line(`Target server ${inactive.name} is not active.`);
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    if (appRow.appKind === "database" && targetServers.length !== 1) {
      log.line("Database apps require exactly one attached server.");
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    if (
      appRow.sourceType === "repository" &&
      appRow.buildRunner === "server" &&
      targetServers.length !== 1
    ) {
      log.line(
        "Selected-server builds require exactly one attached server. Use Depot for multi-server deploys.",
      );
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    let imageRef: string;
    let buildId: string | null = null;
    let registry: { host: string; user: string; token: string } | undefined;
    let pullImage = false;

    if (dep.imageRef) {
      imageRef = dep.imageRef;
      registry = await resolveDepotRegistryForImage(appRow, imageRef);
      log.line(`Using saved deployment image ${imageRef}.`);
    } else if (appRow.appKind === "database") {
      if (appRow.databaseKind !== "postgres") {
        log.line("Only Postgres database apps are supported right now.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      imageRef = appRow.imageRef || databaseImage(appRow);
      if (!imageRef) {
        log.line("No database image configured for this app.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      log.line(`Using managed Postgres image ${imageRef}.`);
    } else if (appRow.sourceType === "image") {
      if (!appRow.imageRef) {
        log.line("No Docker image configured for this app.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      imageRef = appRow.imageRef;
      log.line(`Using prebuilt Docker image ${imageRef}.`);
    } else {
      // Clone.
      log.line(`Cloning ${appRow.repositoryUrl} (${appRow.branch})…`);
      ctxDir = await mkdtemp(join(tmpdir(), "basse-build-"));
      const commitSha = await cloneRepo({
        repositoryUrl: appRow.repositoryUrl,
        branch: appRow.branch,
        ctxDir,
        onLine: log.line,
      });
      await setStatus(deploymentId, "building", { commitSha });

      const kind = resolveBuildKind(appRow.buildMode, ctxDir);

      if (appRow.buildRunner === "server") {
        const buildServer = targetServers[0]!;
        log.line(
          `Building with ${kind === "dockerfile" ? "Dockerfile" : "Railpack"} on ${buildServer.name}…`,
        );
        const connection = await connectionFromServer(buildServer);
        const built = await buildImageOnServer({
          kind,
          ctxDir,
          connection,
          deploymentId,
          onLine: log.line,
        });
        imageRef = built.imageRef;
      } else {
        // Resolve the workspace's Depot connection (token + project + orgId).
        const [env] = await db
          .select()
          .from(environment)
          .where(eq(environment.id, appRow.environmentId))
          .limit(1);
        const [proj] = env
          ? await db.select().from(project).where(eq(project.id, env.projectId)).limit(1)
          : [];
        const [depot] = proj
          ? await db
              .select()
              .from(depotConnection)
              .where(eq(depotConnection.organizationId, proj.organizationId))
              .limit(1)
          : [];

        if (!depot || !depot.orgId) {
          log.line("No Depot connection (token + project id + org id) for this workspace.");
          await log.done();
          await setStatus(deploymentId, "failed");
          return;
        }

        const depotToken = await decryptSecret(depot.token);
        imageRef = `${depot.orgId}.registry.depot.dev/${depot.projectId}:${deploymentId}`;

        // Build remotely on Depot.
        log.line(`Building with ${kind === "dockerfile" ? "Dockerfile" : "Railpack"} on Depot…`);
        const metadataFile = join(ctxDir, "depot-metadata.json");
        const built = await buildImage({
          kind,
          ctxDir,
          depotToken,
          projectId: depot.projectId,
          deploymentId,
          metadataFile,
          onLine: log.line,
        });
        buildId = built.buildId;

        // Mint a pull token (just before the agent call, so it can't expire queued).
        const pullToken = await mintPullToken(depotToken, depot.projectId);
        registry = {
          host: `${depot.orgId}.registry.depot.dev`,
          user: "x-token",
          token: pullToken,
        };
      }
    }

    await setStatus(deploymentId, "deploying", { imageRef, buildId });

    // Decrypt the app's runtime env vars.
    const vars = await db.select().from(envVar).where(eq(envVar.appId, appRow.id));
    const envMap: Record<string, string> = {};
    for (const v of vars) {
      envMap[v.key] = await decryptSecret(v.value);
    }
    if (appRow.appKind === "database") {
      if (!appRow.databasePassword) {
        log.line("Database credentials are missing.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      envMap.POSTGRES_DB = appRow.databaseName ?? "postgres";
      envMap.POSTGRES_USER = appRow.databaseUser ?? "postgres";
      envMap.POSTGRES_PASSWORD = await decryptSecret(appRow.databasePassword);
    }
    const volumes =
      appRow.appKind === "database"
        ? [databaseVolume(appRow.id), ...parseVolumes(appRow.volumes)]
        : parseVolumes(appRow.volumes);

    let allRunning = true;
    for (const srv of targetServers) {
      log.line(`Deploying to ${srv.name}…`);
      const connection = await connectionFromServer(srv);
      const agentToken = await decryptSecret(srv.agentToken!);
      if (appRow.appKind !== "database") {
        await ensureProxy(connection, agentToken);
      }
      if (appRow.sourceType === "image") {
        log.line(`Pulling ${imageRef} on ${srv.name}…`);
        const pull = await runScript(connection, `docker pull ${shellQuote(imageRef)}`, {
          onLine: log.line,
          timeoutMs: 300_000,
        });
        if (pull.exitCode !== 0) {
          throw new Error(`pull image on ${srv.name} failed`);
        }
      }
      const result = await deployApp(connection, agentToken, {
        appId: appRow.id,
        image: imageRef,
        port: appRow.port,
        env: envMap,
        registry,
        pullImage,
        volumes,
        publicPort:
          appRow.appKind === "database" && appRow.databasePublicEnabled
            ? (appRow.databasePublicPort ?? POSTGRES_PORT)
            : undefined,
      });

      log.line(
        result.running
          ? `Container is running on ${srv.name}.`
          : `Container did not start on ${srv.name}.`,
      );
      allRunning &&= result.running;
    }

    await log.done();
    await setStatus(deploymentId, allRunning ? "healthy" : "failed");
    if (allRunning) {
      await db
        .update(deployment)
        .set({ status: "superseded", updatedAt: new Date() })
        .where(
          and(
            eq(deployment.appId, appRow.id),
            ne(deployment.id, deploymentId),
            eq(deployment.status, "healthy"),
          ),
        );
    }
  } catch (error) {
    log.line(`Error: ${error instanceof Error ? error.message : String(error)}`);
    await log.done().catch(() => {});
    await setStatus(deploymentId, "failed").catch(() => {});
  } finally {
    if (ctxDir) await rm(ctxDir, { recursive: true, force: true }).catch(() => {});
  }
}

/**
 * On boot, fail any deployment left mid-flight by a crashed process (its build
 * temp dir and pull token are gone, so it cannot resume).
 */
export async function reconcileInflightDeployments(): Promise<void> {
  await db
    .update(deployment)
    .set({ status: "failed", updatedAt: new Date() })
    .where(inArray(deployment.status, ["building", "deploying"]));
}
