import type { ServerWebSocket } from "bun";
import { app, db, deployment, environment, project, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import { createBunWebSocket } from "hono/bun";
import { resolveActiveWorkspace } from "../lib/workspace";

// Workspace-scoped realtime fan-out over Bun's native WebSockets. Clients
// connect to /api/ws (session cookie auth) and receive small JSON hint events;
// the web app maps them to React Query invalidations, so no entity data ever
// travels over the socket. Publishers never throw — realtime is best effort.
//
// Design rules that keep this from melting the app:
// - Throttling happens BEFORE any database lookup, so a chatty producer (a
//   provision emitting a status per log line) costs at most one org-resolution
//   query per second, not one per line.
// - Events carry the mutating client's id when known, and that client's own
//   sockets are skipped: the actor already has fresh data from the mutation
//   response, so echoing the event back just causes redundant refetches.
// - Org resolution (app/deployment/server -> organizationId) is cached; those
//   relationships are immutable for the lifetime of the row.
//
// The socket registry is in-process. The queue worker, monitor, and backup
// scheduler all run inside the API process (see index.ts), so their events
// reach connected clients directly. A multi-instance deployment would need a
// shared bus (e.g. Redis pub/sub) behind publishRealtime.

export type RealtimeEvent =
  | { type: "deployment"; appId: string }
  | { type: "deployment-logs"; appId: string }
  | { type: "backup"; appId: string }
  | { type: "staged-changes"; appId?: string; projectId?: string; scope: "stage" | "apply" }
  | { type: "alert" }
  | { type: "server"; serverId: string }
  | { type: "domain"; serverId: string };

export type PublishOptions = {
  // Sockets registered by this client id are skipped (originator exclusion).
  excludeClient?: string;
};

type SocketEntry = {
  socket: ServerWebSocket<unknown>;
  clientId: string | null;
};

const socketsByOrg = new Map<string, Set<SocketEntry>>();

function register(organizationId: string, entry: SocketEntry): void {
  let set = socketsByOrg.get(organizationId);
  if (!set) {
    set = new Set();
    socketsByOrg.set(organizationId, set);
  }
  set.add(entry);
}

function unregister(organizationId: string, entry: SocketEntry): void {
  const set = socketsByOrg.get(organizationId);
  if (!set) return;
  set.delete(entry);
  if (set.size === 0) socketsByOrg.delete(organizationId);
}

function sendToOrg(organizationId: string, event: RealtimeEvent, options: PublishOptions): void {
  const set = socketsByOrg.get(organizationId);
  if (!set || set.size === 0) return;
  const message = JSON.stringify(event);
  for (const entry of set) {
    if (options.excludeClient && entry.clientId === options.excludeClient) continue;
    try {
      entry.socket.send(message);
    } catch {
      // A dying socket cleans itself up via onClose.
    }
  }
}

// --- Throttle gate -----------------------------------------------------------
// One fire per key per second, with a trailing fire so the final state always
// goes out. The gate runs before any database work; `fire` does the lookup.

const THROTTLE_MS = 1000;
const lastFiredAt = new Map<string, number>();
const trailing = new Map<string, { timer: ReturnType<typeof setTimeout>; fire: () => void }>();

function gate(key: string, fire: () => void): void {
  const elapsed = Date.now() - (lastFiredAt.get(key) ?? 0);

  if (elapsed < THROTTLE_MS) {
    const pending = trailing.get(key);
    if (pending) {
      // Keep the timer, refresh the payload closure (latest wins).
      pending.fire = fire;
      return;
    }
    trailing.set(key, {
      fire,
      timer: setTimeout(() => {
        const entry = trailing.get(key);
        trailing.delete(key);
        lastFiredAt.set(key, Date.now());
        entry?.fire();
      }, THROTTLE_MS - elapsed),
    });
    return;
  }

  if (lastFiredAt.size > 1000) lastFiredAt.clear();
  lastFiredAt.set(key, Date.now());
  fire();
}

// --- Org resolution cache ----------------------------------------------------

const orgByApp = new Map<string, string>();
const orgByServer = new Map<string, string>();
const appByDeployment = new Map<string, string>();

function cachePut(map: Map<string, string>, key: string, value: string): void {
  if (map.size > 500) map.clear();
  map.set(key, value);
}

async function organizationIdForApp(appId: string): Promise<string | null> {
  const cached = orgByApp.get(appId);
  if (cached) return cached;
  const [row] = await db
    .select({ organizationId: project.organizationId })
    .from(app)
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(app.id, appId))
    .limit(1);
  if (row) cachePut(orgByApp, appId, row.organizationId);
  return row?.organizationId ?? null;
}

// --- Publishers ---------------------------------------------------------------

/** Sends an event to every socket in the workspace. Throttled; never throws. */
export function publishRealtime(
  organizationId: string,
  event: RealtimeEvent,
  options: PublishOptions = {},
): void {
  gate(`${organizationId} ${JSON.stringify(event)}`, () =>
    sendToOrg(organizationId, event, options),
  );
}

/** Resolves the app's workspace and publishes. Fire-and-forget (`void ...`). */
export async function publishForApp(
  appId: string,
  event: RealtimeEvent,
  options: PublishOptions = {},
): Promise<void> {
  gate(`app:${appId} ${JSON.stringify(event)}`, () => {
    void (async () => {
      try {
        const organizationId = await organizationIdForApp(appId);
        if (organizationId) sendToOrg(organizationId, event, options);
      } catch (error) {
        console.error("[realtime]", error);
      }
    })();
  });
}

/**
 * Publishes a deployment hint for the deployment's app. `kind: "status"` marks
 * a lifecycle transition (clients also refresh app/canvas state); `"logs"`
 * marks a build-log append (clients refresh only the deployment list).
 */
export async function publishForDeployment(
  deploymentId: string,
  kind: "status" | "logs" = "status",
): Promise<void> {
  gate(`dep:${deploymentId}:${kind}`, () => {
    void (async () => {
      try {
        let appId = appByDeployment.get(deploymentId);
        if (!appId) {
          const [row] = await db
            .select({ appId: deployment.appId })
            .from(deployment)
            .where(eq(deployment.id, deploymentId))
            .limit(1);
          if (!row) return;
          appId = row.appId;
          cachePut(appByDeployment, deploymentId, appId);
        }
        const organizationId = await organizationIdForApp(appId);
        if (!organizationId) return;
        sendToOrg(
          organizationId,
          kind === "status" ? { type: "deployment", appId } : { type: "deployment-logs", appId },
          {},
        );
      } catch (error) {
        console.error("[realtime]", error);
      }
    })();
  });
}

/** Publishes a server status hint to the server's workspace. */
export async function publishForServer(serverId: string): Promise<void> {
  gate(`srv:${serverId}`, () => {
    void (async () => {
      try {
        let organizationId = orgByServer.get(serverId);
        if (!organizationId) {
          const [row] = await db
            .select({ organizationId: server.organizationId })
            .from(server)
            .where(eq(server.id, serverId))
            .limit(1);
          if (!row) return;
          organizationId = row.organizationId;
          cachePut(orgByServer, serverId, organizationId);
        }
        sendToOrg(organizationId, { type: "server", serverId }, {});
      } catch (error) {
        console.error("[realtime]", error);
      }
    })();
  });
}

// --- WebSocket endpoint --------------------------------------------------------

const { upgradeWebSocket, websocket } = createBunWebSocket();

// The Bun `websocket` handler object index.ts must pass to Bun.serve.
export { websocket };

const CLIENT_ID_PATTERN = /^[a-zA-Z0-9-]{1,64}$/;

export const realtimeRoutes = new Hono<{ Variables: { organizationId: string } }>();

realtimeRoutes.get(
  "/",
  async (c, next) => {
    const organizationId = await resolveActiveWorkspace(c.req.raw);
    if (organizationId instanceof Response) return organizationId;
    c.set("organizationId", organizationId);
    await next();
  },
  upgradeWebSocket((c) => {
    const organizationId = c.get("organizationId") as string;
    const rawClient = c.req.query("client") ?? "";
    const clientId = CLIENT_ID_PATTERN.test(rawClient) ? rawClient : null;
    // Hono constructs a fresh WSContext wrapper per event; the raw Bun socket
    // is the only stable identity, so the registry entry is keyed off it.
    let entry: SocketEntry | null = null;
    return {
      onOpen(_event, socket) {
        entry = { socket: socket.raw as ServerWebSocket<unknown>, clientId };
        register(organizationId, entry);
      },
      onClose() {
        if (entry) unregister(organizationId, entry);
      },
      onMessage(event, socket) {
        // Client keepalive so intermediaries don't idle-close the connection.
        if (event.data === "ping") socket.send("pong");
      },
    };
  }),
);
