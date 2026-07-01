import type { ServerWebSocket } from "bun";
import { app, db, deployment, environment, project, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { Hono } from "hono";
import { createBunWebSocket } from "hono/bun";
import { resolveActiveWorkspace } from "./workspace";

// Workspace-scoped realtime fan-out over Bun's native WebSockets. Clients
// connect to /api/ws (session cookie auth) and receive small JSON hint events;
// the web app maps them to React Query invalidations, so no entity data ever
// travels over the socket. Publishers never throw — realtime is best effort.
//
// The socket registry is in-process. The queue worker, monitor, and backup
// scheduler all run inside the API process (see index.ts), so their events
// reach connected clients directly. A multi-instance deployment would need a
// shared bus (e.g. Redis pub/sub) behind publishRealtime.

export type RealtimeEvent =
  | { type: "deployment"; appId: string }
  | { type: "backup"; appId: string }
  | { type: "staged-changes"; appId?: string; projectId?: string }
  | { type: "alert" }
  | { type: "server"; serverId: string };

// Keyed by the raw Bun socket: Hono constructs a fresh WSContext wrapper per
// event, so only the underlying ServerWebSocket has a stable identity.
const socketsByOrg = new Map<string, Set<ServerWebSocket<unknown>>>();

function register(organizationId: string, socket: ServerWebSocket<unknown>): void {
  let set = socketsByOrg.get(organizationId);
  if (!set) {
    set = new Set();
    socketsByOrg.set(organizationId, set);
  }
  set.add(socket);
}

function unregister(organizationId: string, socket: ServerWebSocket<unknown>): void {
  const set = socketsByOrg.get(organizationId);
  if (!set) return;
  set.delete(socket);
  if (set.size === 0) socketsByOrg.delete(organizationId);
}

/** Sends an event to every socket in the workspace. Never throws. */
export function publishRealtime(organizationId: string, event: RealtimeEvent): void {
  const set = socketsByOrg.get(organizationId);
  if (!set || set.size === 0) return;
  const message = JSON.stringify(event);
  for (const socket of set) {
    try {
      socket.send(message);
    } catch {
      // A dying socket cleans itself up via onClose.
    }
  }
}

/** Resolves the app's workspace and publishes. Fire-and-forget (`void ...`). */
export async function publishForApp(appId: string, event: RealtimeEvent): Promise<void> {
  try {
    const [row] = await db
      .select({ organizationId: project.organizationId })
      .from(app)
      .innerJoin(environment, eq(app.environmentId, environment.id))
      .innerJoin(project, eq(environment.projectId, project.id))
      .where(eq(app.id, appId))
      .limit(1);
    if (row) publishRealtime(row.organizationId, event);
  } catch (error) {
    console.error("[realtime]", error);
  }
}

/** Publishes a deployment status hint for the deployment's app. */
export async function publishForDeployment(deploymentId: string): Promise<void> {
  try {
    const [row] = await db
      .select({ appId: deployment.appId, organizationId: project.organizationId })
      .from(deployment)
      .innerJoin(app, eq(deployment.appId, app.id))
      .innerJoin(environment, eq(app.environmentId, environment.id))
      .innerJoin(project, eq(environment.projectId, project.id))
      .where(eq(deployment.id, deploymentId))
      .limit(1);
    if (row) publishRealtime(row.organizationId, { type: "deployment", appId: row.appId });
  } catch (error) {
    console.error("[realtime]", error);
  }
}

/** Publishes a server status hint to the server's workspace. */
export async function publishForServer(serverId: string): Promise<void> {
  try {
    const [row] = await db
      .select({ organizationId: server.organizationId })
      .from(server)
      .where(eq(server.id, serverId))
      .limit(1);
    if (row) publishRealtime(row.organizationId, { type: "server", serverId });
  } catch (error) {
    console.error("[realtime]", error);
  }
}

const { upgradeWebSocket, websocket } = createBunWebSocket();

// The Bun `websocket` handler object index.ts must pass to Bun.serve.
export { websocket };

export const realtimeRoutes = new Hono<{ Variables: { organizationId: string } }>();

realtimeRoutes.get(
  "/",
  async (c, next) => {
    const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
    if (organizationId instanceof Response) return organizationId;
    c.set("organizationId", organizationId);
    await next();
  },
  upgradeWebSocket((c) => {
    const organizationId = c.get("organizationId") as string;
    return {
      onOpen(_event, socket) {
        register(organizationId, socket.raw as ServerWebSocket<unknown>);
      },
      onClose(_event, socket) {
        unregister(organizationId, socket.raw as ServerWebSocket<unknown>);
      },
      onMessage(event, socket) {
        // Client keepalive so intermediaries don't idle-close the connection.
        if (event.data === "ping") socket.send("pong");
      },
    };
  }),
);
