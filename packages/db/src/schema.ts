import { relations } from "drizzle-orm";
import {
  bigint,
  boolean,
  index,
  integer,
  pgTable,
  text,
  timestamp,
  uniqueIndex,
} from "drizzle-orm/pg-core";

export * from "./auth-schema";
import { organization } from "./auth-schema";

// A user server, workspace-scoped. Basse SSHes in (using its own per-server
// keypair), installs Docker, and runs the Go agent. Secret columns (private
// key, agent token) are encrypted at rest by the API.
export const server = pgTable(
  "server",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    sshHost: text("ssh_host").notNull(),
    sshPort: integer("ssh_port").notNull().default(22),
    sshUser: text("ssh_user").notNull().default("root"),
    // Basse-generated per-server keypair. Public key is shown to the user to
    // paste into authorized_keys; private key is encrypted.
    sshPublicKey: text("ssh_public_key").notNull(),
    sshPrivateKey: text("ssh_private_key").notNull(),
    // Bearer token the agent requires; encrypted at rest. Null until provisioned.
    agentToken: text("agent_token"),
    // Loopback URL the agent listens on (reached via SSH tunnel). Null until up.
    agentUrl: text("agent_url"),
    hostKeyFingerprint: text("host_key_fingerprint"),
    status: text("status", {
      enum: ["pending", "provisioning", "active", "error", "unreachable"],
    })
      .notNull()
      .default("pending"),
    statusMessage: text("status_message"),
    lastSeenAt: timestamp("last_seen_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [index("server_organizationId_idx").on(table.organizationId)],
);

export const project = pgTable(
  "project",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    slug: text("slug").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("project_organizationId_idx").on(table.organizationId),
    uniqueIndex("project_organizationId_slug_uidx").on(table.organizationId, table.slug),
  ],
);

// An environment within a project (e.g. production, staging). Every project has
// a default "production" environment, auto-created on project creation.
export const environment = pgTable(
  "environment",
  {
    id: text("id").primaryKey(),
    projectId: text("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    slug: text("slug").notNull(),
    isDefault: boolean("is_default").notNull().default(false),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("environment_projectId_idx").on(table.projectId),
    uniqueIndex("environment_projectId_slug_uidx").on(table.projectId, table.slug),
  ],
);

export const app = pgTable(
  "app",
  {
    id: text("id").primaryKey(),
    environmentId: text("environment_id")
      .notNull()
      .references(() => environment.id, { onDelete: "cascade" }),
    serverId: text("server_id").references(() => server.id, {
      onDelete: "set null",
    }),
    name: text("name").notNull(),
    slug: text("slug").notNull(),
    repositoryUrl: text("repository_url").notNull(),
    branch: text("branch").notNull().default("main"),
    // The port the app listens on inside its container (the domain upstream).
    port: integer("port").notNull().default(3000),
    buildMode: text("build_mode", {
      enum: ["auto", "dockerfile", "railpack"],
    })
      .notNull()
      .default("auto"),
    buildRunner: text("build_runner", {
      enum: ["depot", "server"],
    })
      .notNull()
      .default("depot"),
    appKind: text("app_kind", {
      enum: ["service", "database"],
    })
      .notNull()
      .default("service"),
    sourceType: text("source_type", {
      enum: ["repository", "image"],
    })
      .notNull()
      .default("repository"),
    imageRef: text("image_ref"),
    volumes: text("volumes").notNull().default("[]"),
    cpuLimitMillicores: integer("cpu_limit_millicores"),
    memoryLimitBytes: bigint("memory_limit_bytes", { mode: "number" }),
    databaseKind: text("database_kind", {
      enum: ["postgres", "redis"],
    }),
    databaseVersion: text("database_version"),
    databaseName: text("database_name"),
    databaseUser: text("database_user"),
    databasePassword: text("database_password"),
    databasePublicEnabled: boolean("database_public_enabled").notNull().default(false),
    databasePublicPort: integer("database_public_port"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("app_environmentId_idx").on(table.environmentId),
    uniqueIndex("app_environmentId_slug_uidx").on(table.environmentId, table.slug),
  ],
);

export const appServer = pgTable(
  "app_server",
  {
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    serverId: text("server_id")
      .notNull()
      .references(() => server.id, { onDelete: "cascade" }),
    createdAt: timestamp("created_at").notNull(),
  },
  (table) => [
    uniqueIndex("app_server_appId_serverId_uidx").on(table.appId, table.serverId),
    index("app_server_appId_idx").on(table.appId),
    index("app_server_serverId_idx").on(table.serverId),
  ],
);

// Runtime environment variables for an app. Values are encrypted at rest.
export const envVar = pgTable(
  "env_var",
  {
    id: text("id").primaryKey(),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    key: text("key").notNull(),
    value: text("value").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("env_var_appId_idx").on(table.appId),
    uniqueIndex("env_var_appId_key_uidx").on(table.appId, table.key),
  ],
);

export const deployment = pgTable("deployment", {
  id: text("id").primaryKey(),
  appId: text("app_id")
    .notNull()
    .references(() => app.id, { onDelete: "cascade" }),
  status: text("status", {
    enum: [
      "queued",
      "building",
      "deploying",
      "healthy",
      "superseded",
      "failed",
      "cancelled",
      "stopped",
    ],
  })
    .notNull()
    .default("queued"),
  commitSha: text("commit_sha"),
  imageRef: text("image_ref"),
  buildId: text("build_id"),
  // Append-only build + deploy log shown in the UI.
  logs: text("logs"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const workspaceSettings = pgTable("workspace_settings", {
  organizationId: text("organization_id")
    .primaryKey()
    .references(() => organization.id, { onDelete: "cascade" }),
  imageRetentionDays: integer("image_retention_days").notNull().default(30),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const projectRelations = relations(project, ({ one, many }) => ({
  organization: one(organization, {
    fields: [project.organizationId],
    references: [organization.id],
  }),
  environments: many(environment),
}));

export const environmentRelations = relations(environment, ({ one, many }) => ({
  project: one(project, {
    fields: [environment.projectId],
    references: [project.id],
  }),
  apps: many(app),
}));

export const serverRelations = relations(server, ({ one, many }) => ({
  organization: one(organization, {
    fields: [server.organizationId],
    references: [organization.id],
  }),
  apps: many(app),
  appServers: many(appServer),
}));

export const workspaceSettingsRelations = relations(workspaceSettings, ({ one }) => ({
  organization: one(organization, {
    fields: [workspaceSettings.organizationId],
    references: [organization.id],
  }),
}));

export const appRelations = relations(app, ({ one, many }) => ({
  environment: one(environment, {
    fields: [app.environmentId],
    references: [environment.id],
  }),
  server: one(server, {
    fields: [app.serverId],
    references: [server.id],
  }),
  deployments: many(deployment),
  envVars: many(envVar),
  appServers: many(appServer),
}));

export const appServerRelations = relations(appServer, ({ one }) => ({
  app: one(app, {
    fields: [appServer.appId],
    references: [app.id],
  }),
  server: one(server, {
    fields: [appServer.serverId],
    references: [server.id],
  }),
}));

export const envVarRelations = relations(envVar, ({ one }) => ({
  app: one(app, {
    fields: [envVar.appId],
    references: [app.id],
  }),
}));

export const deploymentRelations = relations(deployment, ({ one }) => ({
  app: one(app, {
    fields: [deployment.appId],
    references: [app.id],
  }),
}));

// A custom domain routed by the server's Caddy proxy to an upstream
// (container:port or host:port). Workspace-scoped transitively via its server —
// there is NO organizationId column, so every API path MUST join domain->server.
export const domain = pgTable(
  "domain",
  {
    id: text("id").primaryKey(),
    serverId: text("server_id")
      .notNull()
      .references(() => server.id, { onDelete: "cascade" }),
    appId: text("app_id").references(() => app.id, { onDelete: "set null" }),
    host: text("host").notNull(),
    upstream: text("upstream").notNull(),
    status: text("status", { enum: ["pending", "active", "error"] })
      .notNull()
      .default("pending"),
    statusMessage: text("status_message"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("domain_serverId_idx").on(table.serverId),
    uniqueIndex("domain_host_uidx").on(table.host),
  ],
);

export const domainRelations = relations(domain, ({ one }) => ({
  server: one(server, {
    fields: [domain.serverId],
    references: [server.id],
  }),
  app: one(app, {
    fields: [domain.appId],
    references: [app.id],
  }),
}));

export const sshKey = pgTable(
  "ssh_key",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    publicKey: text("public_key").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [index("ssh_key_organizationId_idx").on(table.organizationId)],
);

// One Depot connection per workspace. `token` is encrypted at rest.
export const depotConnection = pgTable("depot_connection", {
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  token: text("token").notNull(),
  projectId: text("project_id").notNull(),
  // Depot organization id — the registry subdomain ({orgId}.registry.depot.dev).
  orgId: text("org_id"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const sshKeyRelations = relations(sshKey, ({ one }) => ({
  organization: one(organization, {
    fields: [sshKey.organizationId],
    references: [organization.id],
  }),
}));

export const depotConnectionRelations = relations(depotConnection, ({ one }) => ({
  organization: one(organization, {
    fields: [depotConnection.organizationId],
    references: [organization.id],
  }),
}));
