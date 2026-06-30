import { Hono } from "hono";
import { serveStatic } from "hono/bun";
import { cors } from "hono/cors";
import { logger } from "hono/logger";
import { auth } from "./auth";
import { depot } from "./depot";
import { projects } from "./projects";
import { servers } from "./servers";
import { sshKeys } from "./ssh-keys";

const app = new Hono();
const webDist = Bun.env.WEB_DIST ?? "./apps/web/dist";
const allowedOrigins = new Set([
  "http://localhost:5173",
  "http://127.0.0.1:5173",
  Bun.env.WEB_ORIGIN,
].filter((origin): origin is string => Boolean(origin)));

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
app.route("/api/ssh-keys", sshKeys);
app.route("/api/depot", depot);
app.route("/api/servers", servers);

app.get("/health", (c) =>
  c.json({
    ok: true,
    service: "basse-api",
  }),
);

app.use("/*", serveStatic({ root: webDist }));
app.get("*", serveStatic({ path: `${webDist}/index.html` }));

export default {
  port: Number(Bun.env.API_PORT ?? 3000),
  hostname: Bun.env.API_HOST ?? "127.0.0.1",
  fetch: app.fetch,
};
