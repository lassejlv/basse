import { Hono } from "hono";
import { serveStatic } from "hono/bun";
import { cors } from "hono/cors";
import { logger } from "hono/logger";
import { runMigrations } from "@basse/db/migrations";
import { alerts } from "./routes/alerts";
import { apiTokens } from "./routes/api-tokens";
import { apps } from "./routes/apps";
import { auth } from "./lib/auth";
import { backups, startBackupScheduler } from "./routes/backups";
import { changes, projectChanges } from "./routes/changes";
import { cronJobs, startCronScheduler } from "./routes/cron-jobs";
import { reconcileInflightDeployments } from "./deploy/deploy";
import { deployments } from "./routes/deployments";
import { depot } from "./routes/depot";
import { domains } from "./routes/domains";
import { environments } from "./routes/environments";
import { envVars } from "./routes/env-vars";
import { github } from "./routes/github";
import { startImagePruner } from "./deploy/image-prune";
import { loadBalancers } from "./routes/load-balancers";
import { startMonitor } from "./infra/monitor";
import { neon } from "./routes/neon";
import { outboundAgent } from "./routes/outbound-agent";
import { projects } from "./routes/projects";
import { realtimeRoutes, websocket } from "./infra/realtime";
import { s3 } from "./routes/s3";
import { actionsQueue } from "./queue/queue";
import { reconcileProvisioningServers } from "./queue/reconcile";
import { startWorker } from "./queue/worker";
import { servers } from "./routes/servers";
import {
  appEnvReferences,
  environmentSharedEnvVars,
  projectSharedEnvVars,
} from "./routes/shared-env-vars";
import { sshKeys } from "./routes/ssh-keys";
import { team } from "./routes/team";
import { workspaceSettingsRoutes } from "./routes/workspace-settings";

if (Bun.env.DB_MIGRATE_ON_STARTUP !== "false") {
  await runMigrations();
  console.log("[db] migrations applied");
}

const app = new Hono();
const webDist = Bun.env.WEB_DIST ?? "./apps/web/dist";
const installScriptUrl =
  Bun.env.INSTALL_SCRIPT_URL ?? "https://raw.githubusercontent.com/lassejlv/basse/main/install.sh";
const updateScriptUrl =
  Bun.env.UPDATE_SCRIPT_URL ?? "https://raw.githubusercontent.com/lassejlv/basse/main/update.sh";
const currentCommitSha = Bun.env.BASSE_COMMIT_SHA ?? Bun.env.BASSE_VERSION ?? "unknown";
const allowedOrigins = new Set(
  ["http://localhost:5173", "http://127.0.0.1:5173", normalizeOrigin(Bun.env.WEB_ORIGIN)].filter(
    (origin): origin is string => Boolean(origin),
  ),
);

function normalizeOrigin(origin?: string): string | undefined {
  if (!origin) return undefined;
  try {
    return new URL(origin).origin;
  } catch {
    return origin;
  }
}

function isCloudRuntime(): boolean {
  return Object.keys(Bun.env).some((key) => key.startsWith("CLOUD_"));
}

function updateCommand(): string {
  return "cd /data/basse && ./update.sh";
}

async function fetchRawScript(url: string, unavailableMessage: string): Promise<Response> {
  const response = await fetch(url, {
    headers: { accept: "text/plain" },
  }).catch(() => null);

  if (!response?.ok) {
    return new Response(`${unavailableMessage}\n`, {
      status: 502,
      headers: { "content-type": "text/plain; charset=utf-8" },
    });
  }

  return new Response(await response.text(), {
    status: 200,
    headers: {
      "cache-control": "public, max-age=300",
      "content-type": "text/plain; charset=utf-8",
    },
  });
}

app.use(logger());
app.use(
  "*",
  cors({
    origin: (origin) => (allowedOrigins.has(origin) ? origin : null),
    allowHeaders: ["content-type", "authorization", "x-basse-client"],
    allowMethods: ["GET", "POST", "PUT", "PATCH", "DELETE", "OPTIONS"],
    credentials: true,
  }),
);

app.on(["GET", "POST"], "/api/auth/*", (c) => auth.handler(c.req.raw));

app.route("/api/projects", projects);
app.route("/api/projects", projectChanges);
app.route("/api/projects", projectSharedEnvVars);
app.route("/api/environments", environments);
app.route("/api/environments", environmentSharedEnvVars);
app.route("/api/apps", apps);
app.route("/api/apps", envVars);
app.route("/api/apps", appEnvReferences);
app.route("/api/apps", changes);
app.route("/api/apps", backups);
app.route("/api/apps", cronJobs);
app.route("/api/deployments", deployments);
app.route("/api/ssh-keys", sshKeys);
app.route("/api/depot", depot);
app.route("/api/neon", neon);
app.route("/api/github", github);
app.route("/api/servers", servers);
app.route("/api/domains", domains);
app.route("/api/load-balancers", loadBalancers);
app.route("/api/s3", s3);
app.route("/api/ws", realtimeRoutes);
app.route("/api/workspace", workspaceSettingsRoutes);
app.route("/api/alerts", alerts);
app.route("/api/api-tokens", apiTokens);
app.route("/api/team", team);
app.route("/api/agent/outbound", outboundAgent);

app.get("/api/system", (c) =>
  c.json({
    mode: isCloudRuntime() ? "cloud" : "self-hosted",
    selfHosted: !isCloudRuntime(),
    currentCommitSha,
    installUrl: "/install",
    updateUrl: "/update",
    updateCommand: updateCommand(),
  }),
);

app.get("/api/system/update-check", async (c) => {
  if (isCloudRuntime()) {
    return c.json({
      mode: "cloud",
      selfHosted: false,
      currentCommitSha,
      latestCommitSha: null,
      updateAvailable: false,
      updateCommand: null,
      message: "Cloud instances are updated by the cloud deploy pipeline.",
    });
  }

  const response = await fetch("https://api.github.com/repos/lassejlv/basse/commits/main", {
    headers: {
      accept: "application/vnd.github+json",
      "user-agent": "basse-update-check",
    },
  }).catch(() => null);

  if (!response?.ok) {
    return c.json(
      {
        error: "Could not check GitHub for the latest version",
        currentCommitSha,
        updateCommand: updateCommand(),
      },
      502,
    );
  }

  const body = (await response.json().catch(() => null)) as { sha?: string } | null;
  const latestCommitSha = body?.sha ?? null;
  const knownCurrent = currentCommitSha !== "unknown" && currentCommitSha.length >= 7;
  const updateAvailable =
    knownCurrent && latestCommitSha
      ? !latestCommitSha.startsWith(currentCommitSha) &&
        !currentCommitSha.startsWith(latestCommitSha)
      : null;

  return c.json({
    mode: "self-hosted",
    selfHosted: true,
    currentCommitSha,
    latestCommitSha,
    updateAvailable,
    updateCommand: updateCommand(),
    message:
      updateAvailable === null
        ? "Current image version is unknown. Run the update command to pull the latest image."
        : updateAvailable
          ? "A newer version is available."
          : "This instance is up to date.",
  });
});

app.get("/health", (c) =>
  c.json({
    ok: true,
    service: "basse-api",
  }),
);

app.get("/install", () => fetchRawScript(installScriptUrl, "install script unavailable"));

app.get("/update", () => fetchRawScript(updateScriptUrl, "update script unavailable"));

app.use("/*", serveStatic({ root: webDist }));
app.get("*", serveStatic({ path: `${webDist}/index.html` }));

// Background services, guarded behind a global symbol: `bun --hot` re-runs
// this module on every save while the previous run's intervals and worker keep
// living, so an unguarded start would stack duplicates (and the reconciles
// would fail every in-flight build on each save). First evaluation wins; a
// full restart picks up code changes to the services.
type BackgroundServices = {
  worker: ReturnType<typeof startWorker>;
  monitor: { close: () => void };
  backupScheduler: { close: () => void };
  cronScheduler: { close: () => void };
  imagePruner: { close: () => void };
};
const SERVICES_KEY = Symbol.for("basse.background-services");
const globalState = globalThis as { [SERVICES_KEY]?: BackgroundServices };

if (!globalState[SERVICES_KEY]) {
  const services: BackgroundServices = {
    // In-process worker that runs background actions (provisioning, deploys, …).
    worker: startWorker(),
    monitor: startMonitor(),
    backupScheduler: startBackupScheduler(),
    cronScheduler: startCronScheduler(),
    imagePruner: startImagePruner(),
  };
  globalState[SERVICES_KEY] = services;

  // Re-enqueue any server left mid-provision by a previous process (crash/restart).
  void reconcileProvisioningServers().catch(() => {});

  // Fail any deployment left mid-build/deploy by a crashed process (can't resume).
  void reconcileInflightDeployments().catch(() => {});

  // Graceful shutdown: stop fetching new jobs and let the in-flight job finish
  // before the process exits (docker-compose grants a stop_grace_period for this).
  let shuttingDown = false;
  const shutdown = async (): Promise<void> => {
    if (shuttingDown) {
      return;
    }
    shuttingDown = true;
    try {
      services.monitor.close();
      services.backupScheduler.close();
      services.cronScheduler.close();
      services.imagePruner.close();
      await services.worker.close();
      await actionsQueue.close();
    } catch (error) {
      console.error("[shutdown]", error);
    } finally {
      process.exit(0);
    }
  };

  process.on("SIGTERM", () => void shutdown());
  process.on("SIGINT", () => void shutdown());

  // PID-1 backstops: a stray rejection/exception must never take the server down.
  process.on("unhandledRejection", (reason) => console.error("[unhandledRejection]", reason));
  process.on("uncaughtException", (error) => console.error("[uncaughtException]", error));
}

export default {
  port: Number(Bun.env.API_PORT ?? 3000),
  hostname: Bun.env.API_HOST ?? "127.0.0.1",
  fetch: app.fetch,
  // Bun's WebSocket handler for /api/ws upgrades (see realtime.ts).
  websocket,
};
