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
  createdAt: string;
  updatedAt: string;
};

export type CreateAppInput = {
  environmentId: string;
  name: string;
  repositoryUrl: string;
  branch?: string;
  port?: number;
  buildMode?: AppBuildMode;
  serverId?: string;
  serverIds?: string[];
};

export type UpdateAppInput = {
  name?: string;
  repositoryUrl?: string;
  branch?: string;
  port?: number;
  buildMode?: AppBuildMode;
  serverId?: string | null;
  serverIds?: string[];
};

// Env var with the value masked (last-4 only). Plaintext never leaves the API.
export type EnvVarMasked = {
  key: string;
  valueHint: string;
  updatedAt: string;
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
  | "cancelled";

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
