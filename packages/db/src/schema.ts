import { relations } from "drizzle-orm";
import { index, pgTable, text, timestamp, uniqueIndex } from "drizzle-orm/pg-core";

export * from "./auth-schema";
import { organization } from "./auth-schema";

export const server = pgTable("server", {
  id: text("id").primaryKey(),
  name: text("name").notNull(),
  agentUrl: text("agent_url").notNull(),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

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
