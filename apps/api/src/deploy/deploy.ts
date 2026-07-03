import {
  app,
  appServer,
  db,
  deployment,
  depotConnection,
  environment,
  project,
  server,
} from "@basse/db";
import type { DatabaseKind } from "@basse/shared";
import { and, eq, inArray, ne } from "drizzle-orm";
import { existsSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import {
  deployApp,
  ensureProxy,
  execAppCommand,
  getAgentInfo,
  getAppStatus,
  type AgentConnection,
} from "../infra/agent-client";
import {
  buildImage,
  buildImageOnServer,
  cloneRepo,
  mintPullToken,
  resolveBuildKind,
} from "./builder";
import { resolveBuildPaths } from "./build-paths";
import { decryptSecret } from "../lib/crypto";
import { notifyDeploymentFinished } from "./deployment-notifications";
import { loadResolvedEnvMap } from "./env-resolver";
import { resolveGitHubCloneToken } from "../routes/github";
import { gitHubHttpsCloneUrl, parseGitHubOwner } from "../integrations/github-utils";
import { waitForHealthy } from "../routes/health";
import { syncServerDomains } from "./proxy-sync";
import { publishForDeployment } from "../infra/realtime";
import { connectionFromServer } from "../infra/server-connection";
import { runScript, type SshConnection } from "../infra/ssh";

type DeploymentRow = typeof deployment.$inferSelect;
type ServerRow = typeof server.$inferSelect;
type AppVolume = { hostPath: string; containerPath: string; readOnly: boolean };

const NON_TERMINAL: DeploymentRow["status"][] = ["queued", "building", "deploying"];
const DEFAULT_POSTGRES_VERSION = "18";
const DEFAULT_REDIS_VERSION = "8";
const POSTGRES_PORT = 5432;
const REDIS_PORT = 6379;

/**
 * Advances a deployment's status, but only while it is still in flight. A
 * deployment that a newer deploy marked cancelled (or that already reached a
 * terminal state) is never resurrected — the stale job's writes are dropped.
 * Returns whether the transition applied, so callers can stop working when
 * they discover they've been cancelled.
 */
async function setStatus(
  id: string,
  status: DeploymentRow["status"],
  extra: Partial<Pick<DeploymentRow, "imageRef" | "buildId" | "commitSha" | "phase">> = {},
): Promise<boolean> {
  const rows = await db
    .update(deployment)
    .set({ status, updatedAt: new Date(), ...extra })
    .where(and(eq(deployment.id, id), inArray(deployment.status, NON_TERMINAL)))
    .returning({ id: deployment.id });
  if (rows.length === 0) return false;
  void publishForDeployment(id);
  if (status === "healthy" || status === "failed") {
    void notifyDeploymentFinished(id, status);
  }
  return true;
}

/** True when a newer deploy cancelled this one mid-run. */
async function isCancelled(id: string): Promise<boolean> {
  const [row] = await db
    .select({ status: deployment.status })
    .from(deployment)
    .where(eq(deployment.id, id))
    .limit(1);
  return !row || row.status === "cancelled";
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
    // Live build logs reach clients via realtime (throttled server-side);
    // there is no client-side polling fallback. "logs" events refresh only the
    // deployment list — not the app/canvas queries that status changes touch.
    void publishForDeployment(deploymentId, "logs");
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

function isSshConnection(connection: AgentConnection): connection is SshConnection {
  return !("mode" in connection);
}

function databaseImage(appRow: typeof app.$inferSelect): string {
  const kind = (appRow.databaseKind ?? "postgres") as DatabaseKind;
  if (kind === "postgres") return `postgres:${appRow.databaseVersion ?? DEFAULT_POSTGRES_VERSION}`;
  return `redis:${appRow.databaseVersion ?? DEFAULT_REDIS_VERSION}`;
}

function databasePort(kind: DatabaseKind): number {
  return kind === "postgres" ? POSTGRES_PORT : REDIS_PORT;
}

function postgresDataPath(version: string | null): string {
  const major = Number.parseInt(version ?? DEFAULT_POSTGRES_VERSION, 10);
  return Number.isFinite(major) && major >= 18 ? "/var/lib/postgresql" : "/var/lib/postgresql/data";
}

function databaseVolume(appId: string, kind: DatabaseKind, version: string | null): AppVolume {
  if (kind === "redis") {
    return {
      hostPath: `basse-redis-${appId}`,
      containerPath: "/data",
      readOnly: false,
    };
  }
  return {
    hostPath: `basse-postgres-${appId}`,
    containerPath: postgresDataPath(version),
    readOnly: false,
  };
}

/** Polls pg_isready inside the container until Postgres accepts connections. */
async function verifyPostgresReady(
  connection: AgentConnection,
  agentToken: string,
  appId: string,
  user: string,
): Promise<{ ok: boolean; output: string }> {
  const userArg = shellQuote(user);
  const command = `
last=''
for attempt in 1 2 3 4 5 6 7 8 9 10; do
  last=$(pg_isready -q -U ${userArg} 2>&1) && exit 0
  sleep 1
done
printf "%s" "$last"
exit 1
`.trim();
  const result = await execAppCommand(connection, agentToken, appId, command);
  return { ok: result.exitCode === 0, output: result.output.trim() };
}

async function verifyRedisAuth(
  connection: AgentConnection,
  agentToken: string,
  appId: string,
  password: string,
): Promise<{ ok: boolean; output: string }> {
  const passwordArg = shellQuote(password);
  const command = `
last=''
for attempt in 1 2 3 4 5; do
  last=$(redis-cli --no-auth-warning --user default -a ${passwordArg} PING 2>&1)
  [ "$last" = "PONG" ] && exit 0
  sleep 1
done
printf "%s" "$last"
exit 1
`.trim();
  const result = await execAppCommand(connection, agentToken, appId, command);
  return { ok: result.exitCode === 0, output: result.output.trim() };
}

/** Maps a Docker engine arch (amd64/x86_64/arm64/aarch64) to a BuildKit platform. */
function platformForArch(arch: string): string | null {
  const normalized = arch.toLowerCase();
  if (normalized === "amd64" || normalized === "x86_64") return "linux/amd64";
  if (normalized === "arm64" || normalized === "aarch64") return "linux/arm64";
  return null;
}

/**
 * Resolves the build platform from the target servers' Docker engines so Depot
 * produces images the servers can actually run (arm64 boxes were previously
 * handed linux/amd64 unconditionally). Mixed-arch fleets are rejected — one
 * image is deployed everywhere. Unknown arches fall back to linux/amd64.
 */
async function resolveBuildPlatform(
  targets: ServerRow[],
  onLine: (line: string) => void,
): Promise<{ platform: string } | { error: string }> {
  const platforms = new Set<string>();
  for (const srv of targets) {
    try {
      const connection = await connectionFromServer(srv);
      const token = await decryptSecret(srv.agentToken!);
      const info = await getAgentInfo(connection, token);
      const platform = platformForArch(info.engine.Arch);
      if (platform) {
        platforms.add(platform);
      } else {
        onLine(`Unknown CPU architecture "${info.engine.Arch}" on ${srv.name}; assuming amd64.`);
        platforms.add("linux/amd64");
      }
    } catch {
      onLine(`Could not detect CPU architecture on ${srv.name}; assuming amd64.`);
      platforms.add("linux/amd64");
    }
  }
  if (platforms.size > 1) {
    return {
      error: `Attached servers have mixed CPU architectures (${[...platforms].join(", ")}). Deploy to servers with a single architecture.`,
    };
  }
  return { platform: [...platforms][0] ?? "linux/amd64" };
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

async function resolveAppOrganizationId(appRow: typeof app.$inferSelect): Promise<string | null> {
  const [row] = await db
    .select({ organizationId: project.organizationId })
    .from(environment)
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(environment.id, appRow.environmentId))
    .limit(1);
  return row?.organizationId ?? null;
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
      .set({ status: "building", phase: "initializing", updatedAt: new Date() })
      .where(and(eq(deployment.id, deploymentId), inArray(deployment.status, ["queued", "failed"])))
      .returning({ id: deployment.id });
    if (!claimed[0]) return;
    void publishForDeployment(deploymentId);

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

    // Resolve env before the build so repository builds can consume it
    // (NEXT_PUBLIC_*/VITE_* style variables are needed at build time).
    let envMap: Record<string, string>;
    try {
      envMap = await loadResolvedEnvMap(appRow);
    } catch (error) {
      log.line(error instanceof Error ? error.message : "Could not resolve environment variables.");
      await log.done();
      await setStatus(deploymentId, "failed");
      return;
    }

    let imageRef: string;
    let buildId: string | null = null;
    let registry: { host: string; user: string; token: string } | undefined;
    let pullImage = false;
    let command: string[] | undefined;
    const databaseKind = (appRow.databaseKind ?? "postgres") as DatabaseKind;

    if (dep.imageRef) {
      imageRef = dep.imageRef;
      buildId = dep.buildId;
      registry = await resolveDepotRegistryForImage(appRow, imageRef);
      log.line(`Using saved deployment image ${imageRef}.`);
    } else if (appRow.appKind === "database") {
      if (!["postgres", "redis"].includes(databaseKind)) {
        log.line("Only Postgres and Redis database apps are supported right now.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      imageRef = appRow.imageRef || databaseImage(appRow);
      pullImage = true;
      if (!imageRef) {
        log.line("No database image configured for this app.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      log.line(
        `Using managed ${databaseKind === "postgres" ? "Postgres" : "Redis"} image ${imageRef}.`,
      );
    } else if (appRow.sourceType === "image") {
      if (!appRow.imageRef) {
        log.line("No Docker image configured for this app.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      imageRef = appRow.imageRef;
      pullImage = true;
      log.line(`Using prebuilt Docker image ${imageRef}.`);
    } else {
      // Clone.
      if (dep.buildNoCache) {
        log.line("Building without cache.");
      }
      log.line(`Cloning ${appRow.repositoryUrl} (${appRow.branch})…`);
      await setStatus(deploymentId, "building", { phase: "cloning" });
      ctxDir = await mkdtemp(join(tmpdir(), "basse-build-"));
      const organizationId = await resolveAppOrganizationId(appRow);
      const gitHubOwner = parseGitHubOwner(appRow.repositoryUrl);
      const authToken = organizationId
        ? await resolveGitHubCloneToken(organizationId, appRow.repositoryUrl)
        : null;
      if (authToken) {
        log.line("Using GitHub App installation token for repository access.");
      } else if (gitHubOwner) {
        log.line(
          `No GitHub App installation matched ${gitHubOwner}. If this repository is private, connect GitHub in Secrets and install the app on the repository.`,
        );
      }
      const cloneUrl = gitHubHttpsCloneUrl(appRow.repositoryUrl) ?? appRow.repositoryUrl;
      let commitSha: string;
      try {
        commitSha = await cloneRepo({
          repositoryUrl: cloneUrl,
          branch: appRow.branch,
          ctxDir,
          authToken,
          onLine: log.line,
        });
      } catch (error) {
        if (authToken && gitHubOwner) {
          log.line(
            `GitHub App authentication was available for ${gitHubOwner}, but cloning failed. Check that the app installation includes this repository and branch.`,
          );
        }
        throw error;
      }
      // Checkpoint before the expensive build: a newer deploy may have
      // cancelled this one while the clone ran.
      if (!(await setStatus(deploymentId, "building", { commitSha, phase: "building" }))) {
        log.line("Deployment was cancelled by a newer deploy.");
        await log.done();
        return;
      }

      const buildPaths = resolveBuildPaths(
        ctxDir,
        appRow.buildRootDirectory,
        appRow.dockerfilePath,
      );
      if (appRow.buildRootDirectory) {
        log.line(`Using build root ${appRow.buildRootDirectory}.`);
      }
      const kind = resolveBuildKind(
        appRow.buildMode,
        buildPaths.buildDir,
        buildPaths.dockerfilePath,
      );
      if (kind === "dockerfile") {
        if (!existsSync(buildPaths.dockerfilePath)) {
          throw new Error(`Dockerfile not found: ${buildPaths.dockerfilePathRelative}`);
        }
        log.line(`Using Dockerfile ${buildPaths.dockerfilePathRelative}.`);
      }

      const buildEnvCount = Object.keys(envMap).length;
      if (buildEnvCount > 0) {
        log.line(`Passing ${buildEnvCount} environment variable(s) to the build.`);
      }

      if (appRow.buildRunner === "server") {
        const buildServer = targetServers[0]!;
        log.line(
          `Building with ${kind === "dockerfile" ? "Dockerfile" : "Railpack"} on ${buildServer.name}…`,
        );
        const connection = await connectionFromServer(buildServer);
        if (!isSshConnection(connection)) {
          log.line("Selected-server builds require SSH. Use Depot builds for outbound servers.");
          await log.done();
          await setStatus(deploymentId, "failed");
          return;
        }
        const built = await buildImageOnServer({
          kind,
          ctxDir: buildPaths.buildDir,
          dockerfilePath: buildPaths.dockerfilePathRelative,
          connection,
          deploymentId,
          buildEnv: envMap,
          noCache: dep.buildNoCache,
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

        const platformResult = await resolveBuildPlatform(targetServers, log.line);
        if ("error" in platformResult) {
          log.line(platformResult.error);
          await log.done();
          await setStatus(deploymentId, "failed");
          return;
        }

        // Build remotely on Depot.
        log.line(
          `Building with ${kind === "dockerfile" ? "Dockerfile" : "Railpack"} on Depot for ${platformResult.platform}…`,
        );
        const metadataFile = join(ctxDir, "depot-metadata.json");
        const built = await buildImage({
          kind,
          ctxDir: buildPaths.buildDir,
          dockerfilePath: buildPaths.dockerfilePath,
          depotToken,
          projectId: depot.projectId,
          deploymentId,
          metadataFile,
          platform: platformResult.platform,
          buildEnv: envMap,
          noCache: dep.buildNoCache,
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

    // Checkpoint after the build: never deploy an image for a cancelled run —
    // the racing job would replace the newer deploy's container.
    if (!(await setStatus(deploymentId, "deploying", { imageRef, buildId, phase: "deploying" }))) {
      log.line("Deployment was cancelled by a newer deploy.");
      await log.done();
      return;
    }

    let databasePasswordPlain: string | null = null;
    if (appRow.appKind === "database") {
      if (!appRow.databasePassword) {
        log.line("Database credentials are missing.");
        await log.done();
        await setStatus(deploymentId, "failed");
        return;
      }
      const password = await decryptSecret(appRow.databasePassword);
      databasePasswordPlain = password;
      if (databaseKind === "postgres") {
        envMap.POSTGRES_DB = appRow.databaseName ?? "postgres";
        envMap.POSTGRES_USER = appRow.databaseUser ?? "postgres";
        envMap.POSTGRES_PASSWORD = password;
      } else {
        command = ["redis-server", "--requirepass", password, "--appendonly", "yes"];
      }
    }
    const volumes =
      appRow.appKind === "database"
        ? [
            databaseVolume(appRow.id, databaseKind, appRow.databaseVersion),
            ...parseVolumes(appRow.volumes),
          ]
        : parseVolumes(appRow.volumes);

    // Fan out to all target servers concurrently: wall-clock is the slowest
    // server, not the sum. Each server's lines are prefixed so interleaved
    // logs stay readable; one server failing doesn't abort the others.
    if (await isCancelled(deploymentId)) {
      log.line("Deployment was cancelled by a newer deploy.");
      await log.done();
      return;
    }

    const deployToServer = async (srv: ServerRow): Promise<boolean> => {
      const line =
        targetServers.length > 1 ? (text: string) => log.line(`[${srv.name}] ${text}`) : log.line;
      if (await isCancelled(deploymentId)) {
        line("Skipped: deployment was cancelled by a newer deploy.");
        return false;
      }
      line(`Deploying to ${srv.name}…`);
      const connection = await connectionFromServer(srv);
      const agentToken = await decryptSecret(srv.agentToken!);
      if (appRow.appKind !== "database") {
        // Caddy must exist before the post-deploy route sync; the sync itself
        // runs only after the new container is up (there is nothing to route
        // to before that).
        await ensureProxy(connection, agentToken);
      }
      if (appRow.sourceType === "image") {
        line(`Pulling ${imageRef} on ${srv.name}…`);
        if (isSshConnection(connection)) {
          const pull = await runScript(connection, `docker pull ${shellQuote(imageRef)}`, {
            onLine: line,
            timeoutMs: 300_000,
          });
          if (pull.exitCode !== 0) {
            throw new Error(`pull image on ${srv.name} failed`);
          }
        }
      }
      const result = await deployApp(connection, agentToken, {
        appId: appRow.id,
        image: imageRef,
        cmd: command,
        port: appRow.port,
        env: envMap,
        registry,
        pullImage,
        volumes,
        cpuLimitMillicores: appRow.cpuLimitMillicores ?? undefined,
        memoryLimitBytes: appRow.memoryLimitBytes ?? undefined,
        publicPort:
          appRow.appKind === "database" && appRow.databasePublicEnabled
            ? (appRow.databasePublicPort ?? databasePort(databaseKind))
            : undefined,
      });

      line(
        result.running
          ? `Container is running on ${srv.name}.`
          : `Container did not start on ${srv.name}.`,
      );
      let running = result.running;

      // Readiness gates: "the process started" is not "the deploy worked".
      if (running && appRow.appKind === "database" && databaseKind === "postgres") {
        line("Waiting for Postgres to accept connections…");
        const ready = await verifyPostgresReady(
          connection,
          agentToken,
          appRow.id,
          appRow.databaseUser ?? "postgres",
        );
        line(
          ready.ok
            ? "Postgres is accepting connections."
            : ready.output
              ? `Postgres did not become ready: ${ready.output}`
              : "Postgres did not become ready.",
        );
        running &&= ready.ok;
      }
      if (
        running &&
        appRow.appKind === "database" &&
        databaseKind === "redis" &&
        databasePasswordPlain
      ) {
        line("Verifying Redis authentication…");
        const auth = await verifyRedisAuth(
          connection,
          agentToken,
          appRow.id,
          databasePasswordPlain,
        );
        if (!auth.ok) {
          line(
            auth.output
              ? `Redis authentication failed: ${auth.output}`
              : "Redis authentication failed.",
          );
        } else {
          line("Redis authentication verified.");
        }
        running &&= auth.ok;
      }
      if (running && appRow.appKind !== "database") {
        if (appRow.healthCheckEnabled) {
          // Health-check-gated cutover: the deploy only goes green (and proxy
          // routes only refresh) once the app answers with the expected status.
          line(
            `Waiting for ${appRow.healthCheckPath} to return ${appRow.healthCheckStatus} on ${srv.name}…`,
          );
          const health = await waitForHealthy(connection, agentToken, appRow, { onLine: line });
          if (health.ok) {
            line(`Health check passed on ${srv.name} (status ${health.status}).`);
          } else {
            line(`Health check failed on ${srv.name}: ${health.reason}`);
            running = false;
          }
        } else {
          // No health check configured: catch immediate crash loops before
          // routing traffic — give the app a moment, then confirm it's up.
          line(`Verifying the container stays up on ${srv.name}…`);
          await Bun.sleep(5000);
          const status = await getAppStatus(connection, agentToken, appRow.id);
          if (!status.running) {
            line(
              `Container exited shortly after starting on ${srv.name} — check the runtime logs.`,
            );
            running = false;
          }
        }
      }

      if (running && appRow.appKind !== "database") {
        line(`Refreshing proxy routes on ${srv.name}…`);
        const sync = await syncServerDomains(srv.id);
        line(
          sync.ok
            ? `Proxy routes refreshed on ${srv.name} (${sync.count}).`
            : `Proxy route refresh failed on ${srv.name}: ${sync.error}`,
        );
      }
      return running;
    };

    const settled = await Promise.allSettled(targetServers.map(deployToServer));
    let allRunning = true;
    settled.forEach((outcome, index) => {
      if (outcome.status === "rejected") {
        const reason =
          outcome.reason instanceof Error ? outcome.reason.message : String(outcome.reason);
        log.line(`Deploy to ${targetServers[index]!.name} failed: ${reason}`);
        allRunning = false;
      } else {
        allRunning &&= outcome.value;
      }
    });

    await log.done();
    const finished = await setStatus(deploymentId, allRunning ? "healthy" : "failed");
    if (allRunning && finished) {
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
