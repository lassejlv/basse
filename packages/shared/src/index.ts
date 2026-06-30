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
export type AppKind = "service" | "database";
export type AppSourceType = "repository" | "image";
export type DatabaseKind = "postgres" | "redis";

export type AppVolume = {
  hostPath: string;
  containerPath: string;
  readOnly: boolean;
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
  buildRunner: AppBuildRunner;
  appKind: AppKind;
  sourceType: AppSourceType;
  imageRef: string | null;
  volumes: AppVolume[];
  database: AppDatabase | null;
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
  buildRunner?: AppBuildRunner;
  appKind?: AppKind;
  sourceType?: AppSourceType;
  imageRef?: string | null;
  volumes?: AppVolume[];
  serverId?: string;
  serverIds?: string[];
  databaseKind?: DatabaseKind;
  databaseVersion?: string;
  databaseName?: string;
  databaseUser?: string;
  databasePassword?: string;
  databasePublicEnabled?: boolean;
  databasePublicPort?: number | null;
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
  name?: string;
  repositoryUrl?: string;
  branch?: string;
  port?: number;
  buildMode?: AppBuildMode;
  buildRunner?: AppBuildRunner;
  appKind?: AppKind;
  sourceType?: AppSourceType;
  imageRef?: string | null;
  volumes?: AppVolume[];
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

export type DeploymentStatus =
  | "queued"
  | "building"
  | "deploying"
  | "healthy"
  | "superseded"
  | "failed"
  | "cancelled"
  | "stopped";

export type Deployment = {
  id: string;
  appId: string;
  status: DeploymentStatus;
  commitSha: string | null;
  imageRef: string | null;
  buildId: string | null;
  logs: string | null;
  createdAt: string;
  updatedAt: string;
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
  createdAt: string;
  updatedAt: string;
};

export type CreateSshKeyInput = {
  name: string;
  publicKey: string;
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

export type ServerStatus = "pending" | "provisioning" | "active" | "error" | "unreachable";

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
  agentUrl: string | null;
  status: ServerStatus;
  statusMessage: string | null;
  hostKeyFingerprint: string | null;
  agentTokenHint?: string;
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
  // Optional: paste an existing private key to reuse one already trusted on the
  // server. When omitted, Basse generates a new per-server keypair.
  privateKey?: string;
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
