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
import { organization, user } from "./auth-schema";

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
    agentTokenHash: text("agent_token_hash"),
    connectionMode: text("connection_mode", { enum: ["ssh", "outbound"] })
      .notNull()
      .default("ssh"),
    // Loopback URL the agent listens on (reached via SSH tunnel). Null until up.
    agentUrl: text("agent_url"),
    isSystem: boolean("is_system").notNull().default(false),
    hostKeyFingerprint: text("host_key_fingerprint"),
    // Set when Basse created the machine on a cloud provider. providerResourceId
    // is the provider's id for it (e.g. the DigitalOcean droplet id).
    provider: text("provider", { enum: ["digitalocean", "hetzner"] }),
    providerResourceId: text("provider_resource_id"),
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

export const agentCommand = pgTable(
  "agent_command",
  {
    id: text("id").primaryKey(),
    serverId: text("server_id")
      .notNull()
      .references(() => server.id, { onDelete: "cascade" }),
    method: text("method").notNull(),
    path: text("path").notNull(),
    body: text("body"),
    status: text("status", { enum: ["queued", "running", "completed", "failed", "expired"] })
      .notNull()
      .default("queued"),
    responseStatus: integer("response_status"),
    responseBody: text("response_body"),
    error: text("error"),
    leaseUntil: timestamp("lease_until"),
    completedAt: timestamp("completed_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("agent_command_serverId_status_idx").on(table.serverId, table.status),
    index("agent_command_createdAt_idx").on(table.createdAt),
  ],
);

export const apiToken = pgTable(
  "api_token",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    createdByUserId: text("created_by_user_id").references(() => user.id, {
      onDelete: "set null",
    }),
    name: text("name").notNull(),
    tokenHash: text("token_hash").notNull(),
    tokenPrefix: text("token_prefix").notNull(),
    scopes: text("scopes").notNull(),
    lastUsedAt: timestamp("last_used_at"),
    expiresAt: timestamp("expires_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("api_token_organizationId_idx").on(table.organizationId),
    uniqueIndex("api_token_tokenHash_uidx").on(table.tokenHash),
  ],
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
    buildRootDirectory: text("build_root_directory").notNull().default(""),
    dockerfilePath: text("dockerfile_path").notNull().default("Dockerfile"),
    buildRunner: text("build_runner", {
      enum: ["depot", "server"],
    })
      .notNull()
      .default("depot"),
    autoRedeployEnabled: boolean("auto_redeploy_enabled").notNull().default(true),
    appKind: text("app_kind", {
      enum: ["service", "database", "neon"],
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
    // Neon-provisioned databases (appKind "neon"). The connection URI is
    // encrypted at rest; the project lives on Neon, not on a Basse server.
    neonProjectId: text("neon_project_id"),
    neonRegion: text("neon_region"),
    neonConnectionUri: text("neon_connection_uri"),
    // HTTP health check (service apps): probed inside the container via the
    // agent (curl/wget), gating deploy cutover and monitored continuously.
    healthCheckEnabled: boolean("health_check_enabled").notNull().default(false),
    healthCheckPath: text("health_check_path").notNull().default("/"),
    healthCheckStatus: integer("health_check_status").notNull().default(200),
    healthCheckTimeoutSeconds: integer("health_check_timeout_seconds").notNull().default(5),
    healthCheckIntervalSeconds: integer("health_check_interval_seconds").notNull().default(30),
    // Scheduled pg_dump backups (postgres databases only).
    backupScheduleEnabled: boolean("backup_schedule_enabled").notNull().default(false),
    backupIntervalHours: integer("backup_interval_hours").notNull().default(24),
    backupRetention: integer("backup_retention").notNull().default(7),
    // When set, completed backups are also uploaded to this S3 connection.
    backupS3ConnectionId: text("backup_s3_connection_id"),
    deployWebhookUrl: text("deploy_webhook_url"),
    deployNotifySuccess: boolean("deploy_notify_success").notNull().default(false),
    deployNotifyFailure: boolean("deploy_notify_failure").notNull().default(false),
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

export const appCronJob = pgTable(
  "app_cron_job",
  {
    id: text("id").primaryKey(),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    command: text("command").notNull(),
    schedule: text("schedule").notNull(),
    enabled: boolean("enabled").notNull().default(true),
    lastStatus: text("last_status", { enum: ["running", "succeeded", "failed"] }),
    lastRunAt: timestamp("last_run_at"),
    lastFinishedAt: timestamp("last_finished_at"),
    lastOutput: text("last_output"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("app_cron_job_appId_idx").on(table.appId),
    index("app_cron_job_enabled_idx").on(table.enabled),
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

// Project-wide variables that apps can reference as {{shared.KEY}}. Values are
// encrypted at rest and resolved into app env values during deploy.
export const projectEnvVar = pgTable(
  "project_env_var",
  {
    id: text("id").primaryKey(),
    projectId: text("project_id")
      .notNull()
      .references(() => project.id, { onDelete: "cascade" }),
    key: text("key").notNull(),
    value: text("value").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("project_env_var_projectId_idx").on(table.projectId),
    uniqueIndex("project_env_var_projectId_key_uidx").on(table.projectId, table.key),
  ],
);

// Environment-specific variables that apps can reference as {{env.KEY}}.
export const environmentEnvVar = pgTable(
  "environment_env_var",
  {
    id: text("id").primaryKey(),
    environmentId: text("environment_id")
      .notNull()
      .references(() => environment.id, { onDelete: "cascade" }),
    key: text("key").notNull(),
    value: text("value").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("environment_env_var_environmentId_idx").on(table.environmentId),
    uniqueIndex("environment_env_var_environmentId_key_uidx").on(table.environmentId, table.key),
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
      // The monitor sets a healthy deployment to "crashed" when its container
      // goes down or crash-loops post-deploy, and back to "healthy" on recovery.
      "crashed",
      "superseded",
      "failed",
      "cancelled",
      "stopped",
    ],
  })
    .notNull()
    .default("queued"),
  // Granular pipeline step within the coarse status, for the UI's stepper.
  phase: text("phase", {
    enum: ["initializing", "cloning", "building", "deploying"],
  }),
  commitSha: text("commit_sha"),
  imageRef: text("image_ref"),
  buildId: text("build_id"),
  buildNoCache: boolean("build_no_cache").notNull().default(false),
  // Append-only build + deploy log shown in the UI.
  logs: text("logs"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

// A workspace-level S3-compatible storage connection (AWS S3, R2, MinIO, …).
// secretAccessKey is encrypted at rest by the API; secretHint keeps last-4 for
// display. Used as an off-server destination for database backups.
export const s3Connection = pgTable(
  "s3_connection",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    name: text("name").notNull(),
    endpoint: text("endpoint"),
    region: text("region"),
    bucket: text("bucket").notNull(),
    accessKeyId: text("access_key_id").notNull(),
    secretAccessKey: text("secret_access_key").notNull(),
    secretHint: text("secret_hint"),
    status: text("status", { enum: ["active", "error"] })
      .notNull()
      .default("active"),
    statusMessage: text("status_message"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [index("s3_connection_organizationId_idx").on(table.organizationId)],
);

export const s3ConnectionRelations = relations(s3Connection, ({ one }) => ({
  organization: one(organization, {
    fields: [s3Connection.organizationId],
    references: [organization.id],
  }),
}));

// A pg_dump backup of a database app. The dump file lives on the target
// server, inside the database's data volume (basse-backups/<id>.dump), so it
// survives container recreation. Workspace-scoped transitively via the app.
export const databaseBackup = pgTable(
  "database_backup",
  {
    id: text("id").primaryKey(),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    serverId: text("server_id")
      .notNull()
      .references(() => server.id, { onDelete: "cascade" }),
    status: text("status", { enum: ["queued", "running", "completed", "failed"] })
      .notNull()
      .default("queued"),
    trigger: text("trigger", { enum: ["manual", "scheduled"] })
      .notNull()
      .default("manual"),
    sizeBytes: bigint("size_bytes", { mode: "number" }),
    error: text("error"),
    // S3 offload state. Null s3Status = never uploaded.
    s3ConnectionId: text("s3_connection_id").references(() => s3Connection.id, {
      onDelete: "set null",
    }),
    s3Status: text("s3_status", { enum: ["uploading", "uploaded", "failed"] }),
    s3Key: text("s3_key"),
    s3Error: text("s3_error"),
    s3UploadedAt: timestamp("s3_uploaded_at"),
    startedAt: timestamp("started_at"),
    completedAt: timestamp("completed_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("database_backup_appId_idx").on(table.appId),
    index("database_backup_appId_status_idx").on(table.appId, table.status),
    index("database_backup_serverId_idx").on(table.serverId),
  ],
);

export const databaseBackupRelations = relations(databaseBackup, ({ one }) => ({
  app: one(app, {
    fields: [databaseBackup.appId],
    references: [app.id],
  }),
  server: one(server, {
    fields: [databaseBackup.serverId],
    references: [server.id],
  }),
}));

export const monitorEvent = pgTable(
  "monitor_event",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    severity: text("severity", { enum: ["info", "warning", "critical"] }).notNull(),
    code: text("code").notNull(),
    title: text("title").notNull(),
    message: text("message").notNull(),
    fingerprint: text("fingerprint").notNull(),
    serverId: text("server_id").references(() => server.id, { onDelete: "cascade" }),
    appId: text("app_id").references(() => app.id, { onDelete: "cascade" }),
    deploymentId: text("deployment_id").references(() => deployment.id, { onDelete: "set null" }),
    createdAt: timestamp("created_at").notNull(),
  },
  (table) => [
    index("monitor_event_organizationId_idx").on(table.organizationId),
    index("monitor_event_fingerprint_idx").on(table.fingerprint),
    index("monitor_event_createdAt_idx").on(table.createdAt),
    index("monitor_event_serverId_idx").on(table.serverId),
    index("monitor_event_appId_idx").on(table.appId),
  ],
);

export const alert = pgTable(
  "alert",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    severity: text("severity", { enum: ["info", "warning", "critical"] }).notNull(),
    status: text("status", { enum: ["open", "acknowledged", "resolved"] })
      .notNull()
      .default("open"),
    code: text("code").notNull(),
    title: text("title").notNull(),
    message: text("message").notNull(),
    fingerprint: text("fingerprint").notNull(),
    serverId: text("server_id").references(() => server.id, { onDelete: "cascade" }),
    appId: text("app_id").references(() => app.id, { onDelete: "cascade" }),
    deploymentId: text("deployment_id").references(() => deployment.id, { onDelete: "set null" }),
    firstSeenAt: timestamp("first_seen_at").notNull(),
    lastSeenAt: timestamp("last_seen_at").notNull(),
    acknowledgedAt: timestamp("acknowledged_at"),
    resolvedAt: timestamp("resolved_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("alert_organizationId_idx").on(table.organizationId),
    index("alert_status_idx").on(table.status),
    index("alert_fingerprint_idx").on(table.fingerprint),
    index("alert_serverId_idx").on(table.serverId),
    index("alert_appId_idx").on(table.appId),
  ],
);

// Railway-style staged ("uncommitted") changes for an app. Each row is one
// pending edit to the app's config, env vars, or domains, not yet applied to the live
// `app`/`env_var`/`domain` tables. They survive page reloads and are applied as a batch
// (then a deploy is triggered) or discarded. Workspace-scoped transitively via
// the app (app->environment->project->organizationId).
export const stagedChange = pgTable(
  "staged_change",
  {
    id: text("id").primaryKey(),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    // What the change targets. "app" = a column on the app row; "env_var" = a
    // runtime environment variable; "domain" = a proxy route.
    resource: text("resource", { enum: ["app", "env_var", "domain"] }).notNull(),
    // For "app" this is always "update". For "env_var" and "domain" it is
    // create/update/delete.
    action: text("action", { enum: ["create", "update", "delete"] }).notNull(),
    // For "app": the app DB column name (e.g. "port", "serverIds"). For
    // "env_var": the variable key. For "domain": `${serverId}:${host}`.
    field: text("field").notNull(),
    // New value. For "app" rows: JSON-encoded column value. For "env_var" rows:
    // the new value, AES-encrypted at rest (null for a delete). For "domain":
    // JSON-encoded route data (null for a delete).
    value: text("value"),
    // Snapshot of the prior value (same encoding as `value`) for diff display;
    // null when the change creates something that did not exist.
    previousValue: text("previous_value"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("staged_change_appId_idx").on(table.appId),
    uniqueIndex("staged_change_appId_resource_field_uidx").on(
      table.appId,
      table.resource,
      table.field,
    ),
  ],
);

// Immutable display snapshots of staged changes after they are applied or
// discarded. Env values are already masked before insertion; do not store
// plaintext secrets here.
export const stagedChangeHistory = pgTable(
  "staged_change_history",
  {
    id: text("id").primaryKey(),
    batchId: text("batch_id").notNull(),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    deploymentId: text("deployment_id").references(() => deployment.id, { onDelete: "set null" }),
    outcome: text("outcome", { enum: ["applied", "discarded"] }).notNull(),
    resource: text("resource", { enum: ["app", "env_var", "domain"] }).notNull(),
    action: text("action", { enum: ["create", "update", "delete"] }).notNull(),
    field: text("field").notNull(),
    value: text("value"),
    previousValue: text("previous_value"),
    stagedAt: timestamp("staged_at").notNull(),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("staged_change_history_appId_idx").on(table.appId),
    index("staged_change_history_batchId_idx").on(table.batchId),
    index("staged_change_history_deploymentId_idx").on(table.deploymentId),
  ],
);

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
  envVars: many(projectEnvVar),
}));

export const environmentRelations = relations(environment, ({ one, many }) => ({
  project: one(project, {
    fields: [environment.projectId],
    references: [project.id],
  }),
  apps: many(app),
  envVars: many(environmentEnvVar),
}));

export const serverRelations = relations(server, ({ one, many }) => ({
  organization: one(organization, {
    fields: [server.organizationId],
    references: [organization.id],
  }),
  apps: many(app),
  appServers: many(appServer),
}));

export const apiTokenRelations = relations(apiToken, ({ one }) => ({
  organization: one(organization, {
    fields: [apiToken.organizationId],
    references: [organization.id],
  }),
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
  cronJobs: many(appCronJob),
  stagedChanges: many(stagedChange),
  stagedChangeHistory: many(stagedChangeHistory),
}));

export const appCronJobRelations = relations(appCronJob, ({ one }) => ({
  app: one(app, {
    fields: [appCronJob.appId],
    references: [app.id],
  }),
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

export const agentCommandRelations = relations(agentCommand, ({ one }) => ({
  server: one(server, {
    fields: [agentCommand.serverId],
    references: [server.id],
  }),
}));

export const envVarRelations = relations(envVar, ({ one }) => ({
  app: one(app, {
    fields: [envVar.appId],
    references: [app.id],
  }),
}));

export const projectEnvVarRelations = relations(projectEnvVar, ({ one }) => ({
  project: one(project, {
    fields: [projectEnvVar.projectId],
    references: [project.id],
  }),
}));

export const environmentEnvVarRelations = relations(environmentEnvVar, ({ one }) => ({
  environment: one(environment, {
    fields: [environmentEnvVar.environmentId],
    references: [environment.id],
  }),
}));

export const deploymentRelations = relations(deployment, ({ one }) => ({
  app: one(app, {
    fields: [deployment.appId],
    references: [app.id],
  }),
}));

export const monitorEventRelations = relations(monitorEvent, ({ one }) => ({
  organization: one(organization, {
    fields: [monitorEvent.organizationId],
    references: [organization.id],
  }),
  server: one(server, {
    fields: [monitorEvent.serverId],
    references: [server.id],
  }),
  app: one(app, {
    fields: [monitorEvent.appId],
    references: [app.id],
  }),
  deployment: one(deployment, {
    fields: [monitorEvent.deploymentId],
    references: [deployment.id],
  }),
}));

export const alertRelations = relations(alert, ({ one }) => ({
  organization: one(organization, {
    fields: [alert.organizationId],
    references: [organization.id],
  }),
  server: one(server, {
    fields: [alert.serverId],
    references: [server.id],
  }),
  app: one(app, {
    fields: [alert.appId],
    references: [app.id],
  }),
  deployment: one(deployment, {
    fields: [alert.deploymentId],
    references: [deployment.id],
  }),
}));

export const stagedChangeRelations = relations(stagedChange, ({ one }) => ({
  app: one(app, {
    fields: [stagedChange.appId],
    references: [app.id],
  }),
}));

export const stagedChangeHistoryRelations = relations(stagedChangeHistory, ({ one }) => ({
  app: one(app, {
    fields: [stagedChangeHistory.appId],
    references: [app.id],
  }),
  deployment: one(deployment, {
    fields: [stagedChangeHistory.deploymentId],
    references: [deployment.id],
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
    uniqueIndex("domain_serverId_host_uidx").on(table.serverId, table.host),
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
    privateKey: text("private_key"),
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

// One Hetzner Cloud connection per workspace. `apiToken` is encrypted at rest.
export const hetznerConnection = pgTable("hetzner_connection", {
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  apiToken: text("api_token").notNull(),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

// One DigitalOcean connection per workspace. `apiToken` is encrypted at rest.
export const digitaloceanConnection = pgTable("digitalocean_connection", {
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  apiToken: text("api_token").notNull(),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

// One Neon connection per workspace. `apiKey` is encrypted at rest.
export const neonConnection = pgTable("neon_connection", {
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  apiKey: text("api_key").notNull(),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const githubAppIntegration = pgTable("github_app_integration", {
  id: text("id").primaryKey(),
  organizationId: text("organization_id")
    .notNull()
    .unique()
    .references(() => organization.id, { onDelete: "cascade" }),
  appId: text("app_id").notNull(),
  slug: text("slug").notNull(),
  name: text("name").notNull(),
  clientId: text("client_id"),
  privateKey: text("private_key").notNull(),
  webhookSecret: text("webhook_secret"),
  createdAt: timestamp("created_at").notNull(),
  updatedAt: timestamp("updated_at").notNull(),
});

export const githubAppInstallation = pgTable(
  "github_app_installation",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    integrationId: text("integration_id")
      .notNull()
      .references(() => githubAppIntegration.id, { onDelete: "cascade" }),
    installationId: text("installation_id").notNull(),
    accountLogin: text("account_login").notNull(),
    accountType: text("account_type"),
    repositorySelection: text("repository_selection"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("github_app_installation_organizationId_idx").on(table.organizationId),
    uniqueIndex("github_app_installation_integrationId_installationId_uidx").on(
      table.integrationId,
      table.installationId,
    ),
  ],
);

export const githubWebhookDelivery = pgTable(
  "github_webhook_delivery",
  {
    id: text("id").primaryKey(),
    integrationId: text("integration_id")
      .notNull()
      .references(() => githubAppIntegration.id, { onDelete: "cascade" }),
    deliveryId: text("delivery_id").notNull(),
    event: text("event").notNull(),
    createdAt: timestamp("created_at").notNull(),
  },
  (table) => [
    index("github_webhook_delivery_integrationId_idx").on(table.integrationId),
    uniqueIndex("github_webhook_delivery_integrationId_deliveryId_uidx").on(
      table.integrationId,
      table.deliveryId,
    ),
  ],
);

// Workspace-level credentials for third-party traffic providers. Tokens are
// encrypted by the API before they reach this table.
export const loadBalancerIntegration = pgTable(
  "load_balancer_integration",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    provider: text("provider", { enum: ["hetzner", "cloudflare"] }).notNull(),
    name: text("name").notNull(),
    token: text("token").notNull(),
    tokenHint: text("token_hint"),
    status: text("status", { enum: ["active", "error"] })
      .notNull()
      .default("active"),
    statusMessage: text("status_message"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("load_balancer_integration_organizationId_idx").on(table.organizationId),
    uniqueIndex("load_balancer_integration_organizationId_provider_uidx").on(
      table.organizationId,
      table.provider,
    ),
  ],
);

// Basse-owned load balancer resources. The provider-specific resource id is
// intentionally opaque; sync code owns translating this row into provider API calls.
export const loadBalancer = pgTable(
  "load_balancer",
  {
    id: text("id").primaryKey(),
    organizationId: text("organization_id")
      .notNull()
      .references(() => organization.id, { onDelete: "cascade" }),
    integrationId: text("integration_id")
      .notNull()
      .references(() => loadBalancerIntegration.id, { onDelete: "cascade" }),
    appId: text("app_id")
      .notNull()
      .references(() => app.id, { onDelete: "cascade" }),
    provider: text("provider", { enum: ["hetzner", "cloudflare"] }).notNull(),
    name: text("name").notNull(),
    host: text("host").notNull(),
    location: text("location").notNull().default("fsn1"),
    loadBalancerType: text("load_balancer_type").notNull().default("lb11"),
    healthCheckPath: text("health_check_path").notNull().default("/"),
    providerResourceId: text("provider_resource_id"),
    endpointIpv4: text("endpoint_ipv4"),
    endpointIpv6: text("endpoint_ipv6"),
    status: text("status", { enum: ["pending", "syncing", "active", "error"] })
      .notNull()
      .default("pending"),
    statusMessage: text("status_message"),
    lastSyncedAt: timestamp("last_synced_at"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("load_balancer_organizationId_idx").on(table.organizationId),
    index("load_balancer_integrationId_idx").on(table.integrationId),
    uniqueIndex("load_balancer_appId_uidx").on(table.appId),
    uniqueIndex("load_balancer_organizationId_host_uidx").on(table.organizationId, table.host),
  ],
);

export const loadBalancerTarget = pgTable(
  "load_balancer_target",
  {
    id: text("id").primaryKey(),
    loadBalancerId: text("load_balancer_id")
      .notNull()
      .references(() => loadBalancer.id, { onDelete: "cascade" }),
    serverId: text("server_id")
      .notNull()
      .references(() => server.id, { onDelete: "cascade" }),
    address: text("address").notNull(),
    providerTargetId: text("provider_target_id"),
    status: text("status", { enum: ["pending", "active", "error"] })
      .notNull()
      .default("pending"),
    statusMessage: text("status_message"),
    createdAt: timestamp("created_at").notNull(),
    updatedAt: timestamp("updated_at").notNull(),
  },
  (table) => [
    index("load_balancer_target_loadBalancerId_idx").on(table.loadBalancerId),
    index("load_balancer_target_serverId_idx").on(table.serverId),
    uniqueIndex("load_balancer_target_loadBalancerId_serverId_uidx").on(
      table.loadBalancerId,
      table.serverId,
    ),
  ],
);

export const loadBalancerEvent = pgTable(
  "load_balancer_event",
  {
    id: text("id").primaryKey(),
    loadBalancerId: text("load_balancer_id")
      .notNull()
      .references(() => loadBalancer.id, { onDelete: "cascade" }),
    status: text("status", { enum: ["info", "success", "error"] }).notNull(),
    message: text("message").notNull(),
    details: text("details"),
    createdAt: timestamp("created_at").notNull(),
  },
  (table) => [
    index("load_balancer_event_loadBalancerId_idx").on(table.loadBalancerId),
    index("load_balancer_event_createdAt_idx").on(table.createdAt),
  ],
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

export const loadBalancerIntegrationRelations = relations(
  loadBalancerIntegration,
  ({ one, many }) => ({
    organization: one(organization, {
      fields: [loadBalancerIntegration.organizationId],
      references: [organization.id],
    }),
    loadBalancers: many(loadBalancer),
  }),
);

export const loadBalancerRelations = relations(loadBalancer, ({ one, many }) => ({
  organization: one(organization, {
    fields: [loadBalancer.organizationId],
    references: [organization.id],
  }),
  integration: one(loadBalancerIntegration, {
    fields: [loadBalancer.integrationId],
    references: [loadBalancerIntegration.id],
  }),
  app: one(app, {
    fields: [loadBalancer.appId],
    references: [app.id],
  }),
  targets: many(loadBalancerTarget),
  events: many(loadBalancerEvent),
}));

export const loadBalancerTargetRelations = relations(loadBalancerTarget, ({ one }) => ({
  loadBalancer: one(loadBalancer, {
    fields: [loadBalancerTarget.loadBalancerId],
    references: [loadBalancer.id],
  }),
  server: one(server, {
    fields: [loadBalancerTarget.serverId],
    references: [server.id],
  }),
}));

export const loadBalancerEventRelations = relations(loadBalancerEvent, ({ one }) => ({
  loadBalancer: one(loadBalancer, {
    fields: [loadBalancerEvent.loadBalancerId],
    references: [loadBalancer.id],
  }),
}));
