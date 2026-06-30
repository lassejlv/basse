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
  /** Last 4 characters of the access token, for display only. */
  tokenHint?: string;
  updatedAt?: string;
};

export type SaveDepotConnectionInput = {
  token: string;
  projectId: string;
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
