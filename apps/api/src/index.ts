import { Hono } from "hono";
import { serveStatic } from "hono/bun";
import { cors } from "hono/cors";
import { logger } from "hono/logger";
import { runMigrations } from "@basse/db/migrations";
import { alerts } from "./alerts";
import { apps } from "./apps";
import { auth } from "./auth";
import { changes, projectChanges } from "./changes";
import { reconcileInflightDeployments } from "./deploy";
import { deployments } from "./deployments";
import { depot } from "./depot";
import { domains } from "./domains";
import { environments } from "./environments";
import { envVars } from "./env-vars";
import { github } from "./github";
import { loadBalancers } from "./load-balancers";
import { startMonitor } from "./monitor";
import { outboundAgent } from "./outbound-agent";
import { projects } from "./projects";
import { actionsQueue } from "./queue/queue";
import { reconcileProvisioningServers } from "./queue/reconcile";
import { startWorker } from "./queue/worker";
import { servers } from "./servers";
import {
  appEnvReferences,
  environmentSharedEnvVars,
  projectSharedEnvVars,
} from "./shared-env-vars";
import { sshKeys } from "./ssh-keys";
import { workspaceSettingsRoutes } from "./workspace-settings";

if (Bun.env.DB_MIGRATE_ON_STARTUP !== "false") {
  await runMigrations();
  console.log("[db] migrations applied");
}

const app = new Hono();
const webDist = Bun.env.WEB_DIST ?? "./apps/web/dist";
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

app.use(logger());
app.use(
  "*",
  cors({
    origin: (origin) => (allowedOrigins.has(origin) ? origin : null),
    allowHeaders: ["content-type", "authorization"],
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
app.route("/api/deployments", deployments);
app.route("/api/ssh-keys", sshKeys);
app.route("/api/depot", depot);
app.route("/api/github", github);
app.route("/api/servers", servers);
app.route("/api/domains", domains);
app.route("/api/load-balancers", loadBalancers);
app.route("/api/workspace", workspaceSettingsRoutes);
app.route("/api/alerts", alerts);
app.route("/api/agent/outbound", outboundAgent);

app.get("/health", (c) =>
  c.json({
    ok: true,
    service: "basse-api",
  }),
);

app.use("/*", serveStatic({ root: webDist }));
app.get("*", serveStatic({ path: `${webDist}/index.html` }));

// Start the in-process worker that runs background actions (provisioning, …).
const worker = startWorker();
const monitor = startMonitor();

// Re-enqueue any server left mid-provision by a previous process (crash/restart).
void reconcileProvisioningServers().catch(() => {});

// Fail any deployment left mid-build/deploy by a crashed process (can't resume).
void reconcileInflightDeployments().catch(() => {});

// Graceful shutdown: stop fetching new jobs and let the in-flight job finish
// before the process exits (docker-compose grants a stop_grace_period for this).
let shuttingDown = false;
async function shutdown(): Promise<void> {
  if (shuttingDown) {
    return;
  }
  shuttingDown = true;
  try {
    monitor.close();
    await worker.close();
    await actionsQueue.close();
  } catch (error) {
    console.error("[shutdown]", error);
  } finally {
    process.exit(0);
  }
}

process.on("SIGTERM", () => void shutdown());
process.on("SIGINT", () => void shutdown());

// PID-1 backstops: a stray rejection/exception must never take the server down.
process.on("unhandledRejection", (reason) => console.error("[unhandledRejection]", reason));
process.on("uncaughtException", (error) => console.error("[uncaughtException]", error));

export default {
  port: Number(Bun.env.API_PORT ?? 3000),
  hostname: Bun.env.API_HOST ?? "127.0.0.1",
  fetch: app.fetch,
};
