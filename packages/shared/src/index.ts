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
