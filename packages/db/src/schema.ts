import { relations } from "drizzle-orm";
import { index, integer, pgTable, text, timestamp, uniqueIndex } from "drizzle-orm/pg-core";

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

export const app = pgTable("app", {
  id: text("id").primaryKey(),
  projectId: text("project_id")
    .notNull()
    .references(() => project.id, { onDelete: "cascade" }),
  serverId: text("server_id").references(() => server.id, {
    onDelete: "set null",
  }),
  name: text("name").notNull(),
  slug: text("slug").notNull().unique(),
  repositoryUrl: text("repository_url").notNull(),
  branch: text("branch").notNull().default("main"),
  buildMode: text("build_mode", {
    enum: ["nixpacks", "dockerfile", "compose", "image", "static"],
  })
    .notNull()
    .default("nixpacks"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const deployment = pgTable("deployment", {
  id: text("id").primaryKey(),
  appId: text("app_id")
    .notNull()
    .references(() => app.id, { onDelete: "cascade" }),
  status: text("status", {
    enum: ["queued", "building", "deploying", "healthy", "failed", "cancelled"],
  })
    .notNull()
    .default("queued"),
  commitSha: text("commit_sha"),
  imageRef: text("image_ref"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const projectRelations = relations(project, ({ one, many }) => ({
  organization: one(organization, {
    fields: [project.organizationId],
    references: [organization.id],
  }),
  apps: many(app),
}));

export const serverRelations = relations(server, ({ one, many }) => ({
  organization: one(organization, {
    fields: [server.organizationId],
    references: [organization.id],
  }),
  apps: many(app),
}));

export const appRelations = relations(app, ({ one, many }) => ({
  project: one(project, {
    fields: [app.projectId],
    references: [project.id],
  }),
  server: one(server, {
    fields: [app.serverId],
    references: [server.id],
  }),
  deployments: many(deployment),
}));

export const deploymentRelations = relations(deployment, ({ one }) => ({
  app: one(app, {
    fields: [deployment.appId],
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
export const depotConnection = pgTable(
  "depot_connection",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .unique()
      .references(() => organization.id, { onDelete: "cascade" }),
    token: text("token").notNull(),
    projectId: text("project_id").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
);

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
