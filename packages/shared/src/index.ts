// API contract types shared between the api and web apps. These describe the
// JSON shapes crossing the network (timestamps are ISO strings), independent of
// the database row types in @basse/db.

export type Project = {
  id: string;
  organizationId: string;
  name: string;
  slug: string;
  createdAt: string;
  updatedAt: string;
};

// Project plus rollup counts, returned by the list endpoint to render cards.
export type ProjectListItem = Project & {
  environmentCount: number;
  appCount: number;
};

export type CreateProjectInput = {
  name: string;
};

export type Environment = {
  id: string;
  projectId: string;
  name: string;
  slug: string;
  isDefault: boolean;
  createdAt: string;
  updatedAt: string;
};

export type CreateEnvironmentInput = {
  projectId: string;
  name: string;
};

export type AppBuildMode = "auto" | "dockerfile" | "railpack";
export type AppBuildRunner = "depot" | "server";
export type AppKind = "service" | "database" | "neon";
export type AppSourceType = "repository" | "image";
export type DatabaseKind = "postgres" | "redis";

export type AppVolume = {
  hostPath: string;
  containerPath: string;
  readOnly: boolean;
};

export type AppResourceLimits = {
  cpuMillicores: number | null;
  memoryBytes: number | null;
};

export type AppHealthCheck = {
  enabled: boolean;
  path: string;
  expectedStatus: number;
  timeoutSeconds: number;
  intervalSeconds: number;
};

export type AppDeployNotifications = {
  webhookUrl: string | null;
  notifyOnSuccess: boolean;
  notifyOnFailure: boolean;
};

export type AppDatabase = {
  kind: DatabaseKind;
  version: string;
  name: string;
  user: string | null;
  internalHost: string;
  internalPort: number;
  publicEnabled: boolean;
  publicPort: number | null;
};

// Neon-provisioned Postgres (appKind "neon"). The connection string itself is
// exposed only through the app's env vars (DATABASE_URL) and the connection
// endpoint, never on the App DTO.
export type AppNeon = {
  projectId: string;
  region: string;
};

export type App = {
  id: string;
  environmentId: string;
  serverIds: string[];
  serverId: string | null;
  name: string;
  slug: string;
  repositoryUrl: string;
  branch: string;
  port: number;
  buildMode: AppBuildMode;
  buildRootDirectory: string;
  dockerfilePath: string;
  buildRunner: AppBuildRunner;
  autoRedeployEnabled: boolean;
  appKind: AppKind;
  sourceType: AppSourceType;
  imageRef: string | null;
  volumes: AppVolume[];
  resourceLimits: AppResourceLimits;
  healthCheck: AppHealthCheck;
  deployNotifications: AppDeployNotifications;
  database: AppDatabase | null;
  neon: AppNeon | null;
  createdAt: string;
  updatedAt: string;
  // Status of the most recent deployment, for at-a-glance health. Present on
  // list/get responses; null when the app has never deployed.
  latestDeploymentStatus?: DeploymentStatus | null;
  // Breadcrumb context, populated on the single-app GET only.
  environmentName?: string;
  projectId?: string;
  projectName?: string;
};

export type CreateAppInput = {
  environmentId: string;
  name: string;
  repositoryUrl?: string;
  branch?: string;
  port?: number;
  buildMode?: AppBuildMode;
  buildRootDirectory?: string;
  dockerfilePath?: string;
  buildRunner?: AppBuildRunner;
  autoRedeployEnabled?: boolean;
  appKind?: AppKind;
  sourceType?: AppSourceType;
  imageRef?: string | null;
  volumes?: AppVolume[];
  cpuLimitMillicores?: number | null;
  memoryLimitBytes?: number | null;
  serverId?: string;
  serverIds?: string[];
  databaseKind?: DatabaseKind;
  databaseVersion?: string;
  databaseName?: string;
  databaseUser?: string;
  databasePassword?: string;
  databasePublicEnabled?: boolean;
  databasePublicPort?: number | null;
  // Neon region id (e.g. "aws-eu-central-1"); required when appKind is "neon".
  neonRegion?: string;
};

export type ImportableDockerContainerPort = {
  ip?: string;
  privatePort: number;
  publicPort?: number;
  type: string;
};

export type ImportableDockerContainer = {
  id: string;
  name: string;
  image: string;
  imageId: string;
  state: string;
  status: string;
  running: boolean;
  ports: ImportableDockerContainerPort[];
};

export type ImportDockerContainerInput = {
  environmentId: string;
  serverId: string;
  containerId: string;
  name: string;
  port: number;
};

export type UpdateAppInput = {
  healthCheckEnabled?: boolean;
  healthCheckPath?: string;
  healthCheckStatus?: number;
  healthCheckTimeoutSeconds?: number;
  healthCheckIntervalSeconds?: number;
  deployWebhookUrl?: string | null;
  deployNotifySuccess?: boolean;
  deployNotifyFailure?: boolean;
  name?: string;
  repositoryUrl?: string;
  branch?: string;
  port?: number;
  buildMode?: AppBuildMode;
  buildRootDirectory?: string;
  dockerfilePath?: string;
  buildRunner?: AppBuildRunner;
  autoRedeployEnabled?: boolean;
  appKind?: AppKind;
  sourceType?: AppSourceType;
  imageRef?: string | null;
  volumes?: AppVolume[];
  cpuLimitMillicores?: number | null;
  memoryLimitBytes?: number | null;
  serverId?: string | null;
  serverIds?: string[];
  databaseVersion?: string;
  databasePublicEnabled?: boolean;
  databasePublicPort?: number | null;
};

export type DatabaseConnectionInfo = {
  internalUri: string;
  publicUri: string | null;
};

export type DatabaseBackupStatus = "queued" | "running" | "completed" | "failed";
export type DatabaseBackupTrigger = "manual" | "scheduled";
export type BackupS3Status = "uploading" | "uploaded" | "failed";

export type DatabaseBackup = {
  id: string;
  appId: string;
  serverId: string;
  status: DatabaseBackupStatus;
  trigger: DatabaseBackupTrigger;
  sizeBytes: number | null;
  error: string | null;
  s3ConnectionId: string | null;
  s3Status: BackupS3Status | null;
  s3Key: string | null;
  s3Error: string | null;
  s3UploadedAt: string | null;
  startedAt: string | null;
  completedAt: string | null;
  createdAt: string;
};

export type DatabaseBackupSettings = {
  scheduleEnabled: boolean;
  intervalHours: number;
  retention: number;
  s3ConnectionId: string | null;
};

export type DatabaseBackupList = {
  backups: DatabaseBackup[];
  settings: DatabaseBackupSettings;
};

export type UpdateDatabaseBackupSettingsInput = {
  scheduleEnabled?: boolean;
  intervalHours?: number;
  retention?: number;
  // null clears the destination.
  s3ConnectionId?: string | null;
};

export type S3ConnectionStatus = "active" | "error";

export type S3Connection = {
  id: string;
  name: string;
  endpoint: string | null;
  region: string | null;
  bucket: string;
  accessKeyId: string;
  secretHint: string | null;
  status: S3ConnectionStatus;
  statusMessage: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CreateS3ConnectionInput = {
  name: string;
  endpoint?: string;
  region?: string;
  bucket: string;
  accessKeyId: string;
  secretAccessKey: string;
};

export type UpdateS3ConnectionInput = Partial<CreateS3ConnectionInput>;

// Env var with the value masked (last-4 only), returned by the list endpoint.
export type EnvVarMasked = {
  key: string;
  valueHint: string;
  updatedAt: string;
};

// A decrypted key/value pair, returned only by the explicit reveal endpoint so
// the user can view, copy, and round-trip-edit their own workspace's secrets.
export type EnvVarPlain = {
  key: string;
  value: string;
};

export type SetEnvVarsInput = {
  vars: { key: string; value: string }[];
};

export type SharedEnvVarScope = "shared" | "env";

export type SharedEnvVarMasked = {
  key: string;
  valueHint: string;
  updatedAt: string;
};

export type SharedEnvVarPlain = {
  key: string;
  value: string;
};

export type SetSharedEnvVarsInput = {
  vars: { key: string; value: string }[];
};

export type EnvReferenceSuggestion = {
  scope: SharedEnvVarScope;
  key: string;
  insertText: string;
  label: string;
  valueHint: string;
  environmentId?: string;
  environmentName?: string;
};

// ── Staged ("uncommitted") changes ───────────────────────────────────────────
// A Railway-style staging area: edits to an app's config or env vars are held
// here until the user applies them (which commits them and triggers a deploy)
// or discards them. They are persisted server-side, so they survive reloads.

export type StagedChangeResource = "app" | "env_var" | "domain";
export type StagedChangeAction = "create" | "update" | "delete";

export type StagedChange = {
  id: string;
  appId: string;
  resource: StagedChangeResource;
  action: StagedChangeAction;
  // For "app": the app field name (e.g. "port", "serverIds"). For "env_var": the
  // variable key. For "domain": `${serverId}:${host}`.
  field: string;
  // Display-safe representation of the new value. For "app" rows it is the
  // JSON-encoded column value; for "env_var" rows it is a masked hint (never
  // plaintext) or null on delete; for "domain" rows it is a JSON-encoded route
  // or null on delete.
  value: string | null;
  // Display-safe representation of the prior value (same encoding as `value`),
  // or null when the change creates something new.
  previousValue: string | null;
  createdAt: string;
};

export type StagedChangeHistoryOutcome = "applied" | "discarded";

export type StagedChangeHistoryItem = {
  id: string;
  batchId: string;
  appId: string;
  deploymentId: string | null;
  outcome: StagedChangeHistoryOutcome;
  resource: StagedChangeResource;
  action: StagedChangeAction;
  field: string;
  value: string | null;
  previousValue: string | null;
  stagedAt: string;
  createdAt: string;
};

export type StagedChangeHistoryEntry = {
  id: string;
  appId: string;
  deploymentId: string | null;
  outcome: StagedChangeHistoryOutcome;
  createdAt: string;
  changes: StagedChangeHistoryItem[];
};

export type ProjectStagedChange = StagedChange & {
  appName: string;
  appId: string;
  environmentId: string;
  environmentName: string;
};

export type ProjectStagedChanges = {
  changes: ProjectStagedChange[];
};

export type ProjectStagedChangesResult = {
  changes: ProjectStagedChange[];
};

export type ProjectApplyStagedChangesResult = {
  deployments: {
    appId: string;
    appName: string;
    deployment: Deployment | null;
    domainSyncs: number;
  }[];
};

export type ProjectStagedChangeHistoryEntry = StagedChangeHistoryEntry & {
  appName: string;
  environmentId: string;
  environmentName: string;
};

// The full staged state for an app: the list of pending changes plus the draft
// App (current config overlaid with the staged app-config changes) so config
// forms can seed their fields from what the user will deploy.
export type AppStagedChanges = {
  changes: StagedChange[];
  draft: App;
};

// Stage a partial set of app-config edits (same shape as a PATCH body); the
// server diffs them against the live app and records only what actually differs.
export type StageAppChangesInput = UpdateAppInput;

// Stage the full desired env-var set; the server diffs it against the live vars.
export type StageEnvVarsInput = SetEnvVarsInput;

export type StageDomainChangeInput =
  | {
      action: "create";
      serverId: string;
      host: string;
      upstream: string;
    }
  | {
      action: "delete";
      domainId: string;
    };

export type PreviewDomainConfig = {
  enabled: boolean;
  rootDomain: string | null;
  host: string | null;
};

// Result of applying staged changes: the deployment that was triggered, or null
// when the app has no server attached and could not be deployed.
export type ApplyStagedChangesResult = {
  deployment: Deployment | null;
  domainSyncs: number;
};

export type DeploymentStatus =
  | "queued"
  | "building"
  | "deploying"
  | "healthy"
  // Set by the monitor when a healthy app's container goes down or
  // crash-loops; cleared back to "healthy" on recovery.
  | "crashed"
  | "superseded"
  | "failed"
  | "cancelled"
  | "stopped";

export type DeploymentPhase = "initializing" | "cloning" | "building" | "deploying";

export type Deployment = {
  id: string;
  appId: string;
  status: DeploymentStatus;
  // Granular pipeline step for the stepper UI; null on legacy rows.
  phase: DeploymentPhase | null;
  commitSha: string | null;
  imageRef: string | null;
  buildId: string | null;
  buildNoCache: boolean;
  logs: string | null;
  createdAt: string;
  updatedAt: string;
};

export type TriggerDeploymentInput = {
  appId: string;
  useLatestImage?: boolean;
  noCache?: boolean;
};

export type CronJobStatus = "running" | "succeeded" | "failed";

export type CronJob = {
  id: string;
  appId: string;
  name: string;
  command: string;
  schedule: string;
  enabled: boolean;
  lastStatus: CronJobStatus | null;
  lastRunAt: string | null;
  lastFinishedAt: string | null;
  lastOutput: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CreateCronJobInput = {
  name: string;
  command: string;
  schedule: string;
  enabled?: boolean;
};

export type UpdateCronJobInput = Partial<CreateCronJobInput>;

export type MonitorSeverity = "info" | "warning" | "critical";
export type AlertStatus = "open" | "acknowledged" | "resolved";

export type Alert = {
  id: string;
  organizationId: string;
  severity: MonitorSeverity;
  status: AlertStatus;
  code: string;
  title: string;
  message: string;
  fingerprint: string;
  serverId: string | null;
  serverName: string | null;
  appId: string | null;
  appName: string | null;
  deploymentId: string | null;
  firstSeenAt: string;
  lastSeenAt: string;
  acknowledgedAt: string | null;
  resolvedAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type MonitorEvent = {
  id: string;
  organizationId: string;
  severity: MonitorSeverity;
  code: string;
  title: string;
  message: string;
  fingerprint: string;
  serverId: string | null;
  serverName: string | null;
  appId: string | null;
  appName: string | null;
  deploymentId: string | null;
  createdAt: string;
};

export type AlertsOverview = {
  openCount: number;
  acknowledgedCount: number;
  criticalOpenCount: number;
};

export type RollbackDeploymentInput = {
  deploymentId: string;
};

export type WorkspaceSettings = {
  organizationId: string;
  imageRetentionDays: number;
  createdAt: string;
  updatedAt: string;
};

export type UpdateWorkspaceSettingsInput = {
  imageRetentionDays: number;
};

export type ApiTokenScope = "read" | "deployments:write" | "write";

export type ApiToken = {
  id: string;
  organizationId: string;
  name: string;
  tokenPrefix: string;
  scopes: ApiTokenScope[];
  lastUsedAt: string | null;
  expiresAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CreateApiTokenInput = {
  name: string;
  scopes: ApiTokenScope[];
  expiresAt?: string | null;
};

export type CreateApiTokenResult = {
  token: string;
  apiToken: ApiToken;
};

export type WorkspaceRole = "owner" | "admin" | "member";

export type TeamMember = {
  id: string;
  userId: string;
  organizationId: string;
  name: string;
  email: string;
  role: WorkspaceRole;
  createdAt: string;
};

export type TeamInvitation = {
  id: string;
  organizationId: string;
  email: string;
  role: WorkspaceRole;
  status: string;
  expiresAt: string;
  createdAt: string;
};

export type TeamOverview = {
  members: TeamMember[];
  invitations: TeamInvitation[];
};

export type InviteTeamMemberInput = {
  email: string;
  role: WorkspaceRole;
};

export type UpdateTeamMemberInput = {
  role: WorkspaceRole;
};

export type AppMetrics = {
  timestamp: string;
  cpuPercent: number;
  memoryBytes: number;
  memoryLimitBytes: number;
  memoryPercent: number;
};

export type AppConsoleResult = {
  exitCode: number;
  output: string;
};

export type AppLogs = {
  logs: string;
};

export type SshKey = {
  id: string;
  organizationId: string;
  name: string;
  publicKey: string;
  hasPrivateKey: boolean;
  createdAt: string;
  updatedAt: string;
};

export type CreateSshKeyInput = {
  name: string;
  privateKey: string;
};

export type DepotConnection = {
  connected: boolean;
  projectId?: string;
  /** Depot organization id — the {orgId}.registry.depot.dev subdomain. */
  orgId?: string | null;
  /** Last 4 characters of the access token, for display only. */
  tokenHint?: string;
  updatedAt?: string;
};

export type SaveDepotConnectionInput = {
  token: string;
  projectId: string;
  orgId: string;
};

export type HetznerConnection = {
  connected: boolean;
  /** Last 4 characters of the API token, for display only. */
  tokenHint?: string;
  updatedAt?: string;
};

export type SaveHetznerConnectionInput = {
  apiToken: string;
};

export type HetznerLocation = {
  slug: string;
  name: string;
  city: string;
  country: string;
};

export type HetznerServerType = {
  slug: string;
  description: string;
  cores: number;
  /** RAM in GB. */
  memory: number;
  /** Disk in GB. */
  disk: number;
  architecture: string;
  /** Location slugs this type is available in, with monthly gross price. */
  prices: { location: string; priceMonthly: number }[];
};

export type CreateHetznerServerInput = {
  name: string;
  location: string;
  serverType: string;
};

export type DigitalOceanConnection = {
  connected: boolean;
  /** Last 4 characters of the API token, for display only. */
  tokenHint?: string;
  updatedAt?: string;
};

export type SaveDigitalOceanConnectionInput = {
  apiToken: string;
};

export type DigitalOceanRegion = {
  slug: string;
  name: string;
  /** Size slugs available in this region. */
  sizes: string[];
};

export type DigitalOceanSize = {
  slug: string;
  description: string;
  vcpus: number;
  /** RAM in MB. */
  memory: number;
  /** Disk in GB. */
  disk: number;
  priceMonthly: number;
  /** Region slugs this size is available in. */
  regions: string[];
};

export type CreateDigitalOceanServerInput = {
  name: string;
  region: string;
  size: string;
};

export type NeonConnection = {
  connected: boolean;
  /** Last 4 characters of the API key, for display only. */
  keyHint?: string;
  updatedAt?: string;
};

export type SaveNeonConnectionInput = {
  apiKey: string;
};

export type NeonRegion = {
  id: string;
  name: string;
};

export type NeonBranch = {
  id: string;
  name: string;
  isDefault: boolean;
  currentState: string | null;
  createdAt: string;
};

export type CreateNeonBranchInput = {
  name: string;
};

// Connection strings for one branch. The pooled URI goes through Neon's
// connection pooler (use it from apps); the direct URI is for migrations and
// tools that need session semantics.
export type NeonBranchConnection = {
  pooledUri: string;
  directUri: string;
  database: string;
  role: string;
};

export type GitHubAppIntegration = {
  connected: boolean;
  appName?: string;
  appSlug?: string;
  appId?: string;
  installUrl?: string;
  webhookUrl?: string;
  updatedAt?: string;
};

export type GitHubAppManifest = {
  actionUrl: string;
  manifest: string;
  state: string;
  webhookUrl: string;
};

export type CompleteGitHubAppManifestInput = {
  code: string;
  state: string;
};

export type GitHubAppInstallation = {
  id: string;
  installationId: string;
  accountLogin: string;
  accountType: string | null;
  repositorySelection: string | null;
  createdAt: string;
  updatedAt: string;
};

export type SaveGitHubAppInstallationInput = {
  installationId: string;
};

export type GitHubRepository = {
  id: string;
  installationId: string;
  accountLogin: string;
  fullName: string;
  cloneUrl: string;
  defaultBranch: string;
  private: boolean;
};

export type GitHubRepositoryList = {
  repositories: GitHubRepository[];
  errors: string[];
};

export type ServerStatus = "pending" | "provisioning" | "active" | "error" | "unreachable";
export type ServerConnectionMode = "ssh" | "outbound";

// Server DTO returned to the client. Secrets (private key, raw agent token) are
// never included — only the public key and a last-4 token hint.
export type Server = {
  id: string;
  organizationId: string;
  name: string;
  sshHost: string;
  sshPort: number;
  sshUser: string;
  sshPublicKey: string;
  connectionMode: ServerConnectionMode;
  agentUrl: string | null;
  isSystem: boolean;
  status: ServerStatus;
  statusMessage: string | null;
  hostKeyFingerprint: string | null;
  /** Set when Basse created the machine on a cloud provider. */
  provider: "digitalocean" | "hetzner" | null;
  /** The provider's id for the machine (e.g. the droplet id). */
  providerResourceId: string | null;
  agentTokenHint?: string;
  // Returned only immediately after creating an outbound server.
  agentInstallCommand?: string;
  lastSeenAt: string | null;
  createdAt: string;
  updatedAt: string;
};

export type AgentInfo = {
  reachable: boolean;
  ready: boolean;
  version?: string;
  targetImage: string;
  docker?: {
    containers: number;
    containersRunning: number;
    images: number;
    ncpu: number;
    memTotal: number;
  };
  engine?: {
    version: string;
    apiVersion: string;
    os: string;
    arch: string;
  };
  error?: string;
};

export type AgentUpdateCheck = {
  targetImage: string;
  currentImageId: string | null;
  latestImageId: string | null;
  updateAvailable: boolean;
  output: string;
};

export type AgentMetrics = {
  timestamp: string;
  cpuPercent: number;
  memoryBytes: number;
  memoryLimitBytes: number;
  memoryPercent: number;
};

export type AgentLogs = {
  logs: string;
};

export type CreateServerInput = {
  name: string;
  sshHost: string;
  sshPort?: number;
  sshUser?: string;
  connectionMode?: ServerConnectionMode;
  sshKeyId?: string;
  // Optional: paste an existing private key to reuse one already trusted on the
  // server. When omitted, Basse generates a new per-server keypair.
  privateKey?: string;
};

export type ServerInstallCommand = {
  agentInstallCommand: string;
};

export type DomainStatus = "pending" | "active" | "error";

// A custom domain routed by a server's Caddy proxy to an upstream.
export type Domain = {
  id: string;
  serverId: string;
  appId: string | null;
  host: string;
  upstream: string;
  status: DomainStatus;
  statusMessage: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CreateDomainInput = {
  host: string;
  upstream: string;
  appId?: string;
};

// The desired-route shape pushed to the agent's /v1/proxy/sync (full set).
export type DesiredDomain = {
  host: string;
  upstream: string;
  appId?: string | null;
};

// Reported by the agent's /v1/proxy/status.
export type ProxyStatus = {
  running: boolean;
  adminReachable: boolean;
  caddyVersion?: string;
};

export type LoadBalancerProvider = "hetzner" | "cloudflare";
export type LoadBalancerIntegrationStatus = "active" | "error";

export type LoadBalancerIntegration = {
  id: string;
  organizationId: string;
  provider: LoadBalancerProvider;
  name: string;
  tokenHint: string | null;
  status: LoadBalancerIntegrationStatus;
  statusMessage: string | null;
  createdAt: string;
  updatedAt: string;
};

export type CreateLoadBalancerIntegrationInput = {
  provider: LoadBalancerProvider;
  name: string;
  token: string;
};

export type ManagedLoadBalancerStatus = "pending" | "syncing" | "active" | "error";
export type LoadBalancerEventStatus = "info" | "success" | "error";

export type ManagedLoadBalancerTarget = {
  id: string;
  serverId: string;
  address: string;
  providerTargetId: string | null;
  status: "pending" | "active" | "error";
  statusMessage: string | null;
  createdAt: string;
  updatedAt: string;
};

export type ManagedLoadBalancer = {
  id: string;
  organizationId: string;
  integrationId: string;
  appId: string;
  provider: LoadBalancerProvider;
  name: string;
  host: string;
  location: string;
  loadBalancerType: string;
  healthCheckPath: string;
  providerResourceId: string | null;
  endpointIpv4: string | null;
  endpointIpv6: string | null;
  status: ManagedLoadBalancerStatus;
  statusMessage: string | null;
  lastSyncedAt: string | null;
  targets: ManagedLoadBalancerTarget[];
  createdAt: string;
  updatedAt: string;
};

export type LoadBalancerEvent = {
  id: string;
  loadBalancerId: string;
  status: LoadBalancerEventStatus;
  message: string;
  details: string | null;
  createdAt: string;
};

export type CreateManagedLoadBalancerInput = {
  appId: string;
  integrationId: string;
  host: string;
  name?: string;
  location?: string;
  loadBalancerType?: string;
  healthCheckPath?: string;
};
