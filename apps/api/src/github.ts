import {
  app as appTable,
  db,
  environment,
  githubAppInstallation,
  githubAppIntegration,
  githubWebhookDelivery,
  project,
} from "@basse/db";
import type {
  CompleteGitHubAppManifestInput,
  GitHubAppInstallation,
  GitHubAppIntegration,
  GitHubAppManifest,
  GitHubRepository,
  GitHubRepositoryList,
  SaveGitHubAppInstallationInput,
} from "@basse/shared";
import { and, eq, sql } from "drizzle-orm";
import { Hono } from "hono";
import { createSign } from "node:crypto";
import { decryptSecret, encryptSecret } from "./crypto";
import {
  createGitHubAppManifest,
  createGitHubManifestState,
  gitHubPushTarget,
  gitHubRepositoryFullName,
  gitHubWebhookUrl,
  parseGitHubOwner,
  resolveApiOrigin,
  resolveManifestOrigin,
  verifyGitHubManifestState,
  verifyWebhookSignature,
} from "./github-utils";
import { enqueueDeploy } from "./deployments";
import { resolveActiveWorkspace } from "./workspace";

const GITHUB_API = "https://api.github.com";
const GITHUB_APP_CREATE_URL = "https://github.com/settings/apps/new";
const GITHUB_API_VERSION = "2022-11-28";

type GitHubIntegrationRow = typeof githubAppIntegration.$inferSelect;
type GitHubInstallationRow = typeof githubAppInstallation.$inferSelect;

type GitHubManifestConversion = {
  id: number | string;
  slug: string;
  name: string;
  client_id?: string;
  pem: string;
  webhook_secret?: string;
};

type GitHubInstallationResponse = {
  id: number | string;
  account: {
    login: string;
    type?: string;
  } | null;
  repository_selection?: string;
};

type InstallationTokenResponse = {
  token: string;
};

type GitHubRepositoryResponse = {
  repositories: {
    id: number | string;
    full_name: string;
    clone_url: string;
    default_branch?: string;
    private?: boolean;
  }[];
};

type GitHubError = {
  message?: string;
};

type GitHubPushPayload = {
  ref?: string;
  deleted?: boolean;
  repository?: {
    full_name?: string;
    clone_url?: string;
    html_url?: string;
  };
};

type GitHubWebhookPayload = GitHubPushPayload & {
  action?: string;
  installation?: {
    id?: number | string;
  };
};

export const github = new Hono();

github.get("/integration", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const integration = await getIntegration(organizationId);
  return c.json(
    toIntegration(
      integration,
      resolveRequestOrigin(c.req.url, c.req.header("origin")),
      resolveApiOrigin(c.req.url, c.req.raw.headers),
    ),
  );
});

github.get("/manifest", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  c.header("Cache-Control", "no-store");
  return c.json(
    createManifest(organizationId, c.req.url, c.req.header("origin"), c.req.raw.headers),
  );
});

github.post("/manifest/complete", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req
    .json()
    .catch(() => null)) as Partial<CompleteGitHubAppManifestInput> | null;
  const code = typeof body?.code === "string" ? body.code.trim() : "";
  if (!code) return c.json({ error: "code is required" }, 400);
  const state = typeof body?.state === "string" ? body.state.trim() : "";
  if (!state || !verifyGitHubManifestState(state, organizationId)) {
    return c.json({ error: "Invalid GitHub App setup state" }, 400);
  }

  const convertedResult = await tryGitHub(() =>
    githubRequest<GitHubManifestConversion>(
      "POST",
      `/app-manifests/${encodeURIComponent(code)}/conversions`,
    ),
  );
  if (!convertedResult.ok) return c.json({ error: convertedResult.error }, 502);

  const converted = convertedResult.value;
  const now = new Date();
  const encryptedPrivateKey = await encryptSecret(converted.pem);
  const encryptedWebhookSecret = converted.webhook_secret
    ? await encryptSecret(converted.webhook_secret)
    : null;

  const [existing] = await db
    .select()
    .from(githubAppIntegration)
    .where(eq(githubAppIntegration.organizationId, organizationId))
    .limit(1);

  const row = await db.transaction(async (tx) => {
    if (existing) {
      await tx
        .delete(githubAppInstallation)
        .where(eq(githubAppInstallation.integrationId, existing.id));
      const [updated] = await tx
        .update(githubAppIntegration)
        .set({
          appId: String(converted.id),
          slug: converted.slug,
          name: converted.name,
          clientId: converted.client_id ?? null,
          privateKey: encryptedPrivateKey,
          webhookSecret: encryptedWebhookSecret,
          updatedAt: now,
        })
        .where(eq(githubAppIntegration.id, existing.id))
        .returning();
      return updated;
    }

    const [created] = await tx
      .insert(githubAppIntegration)
      .values({
        id: crypto.randomUUID(),
        organizationId,
        appId: String(converted.id),
        slug: converted.slug,
        name: converted.name,
        clientId: converted.client_id ?? null,
        privateKey: encryptedPrivateKey,
        webhookSecret: encryptedWebhookSecret,
        createdAt: now,
        updatedAt: now,
      })
      .returning();
    return created;
  });

  if (!row) return c.json({ error: "Failed to save GitHub App" }, 500);
  return c.json(
    toIntegration(
      row,
      resolveRequestOrigin(c.req.url, c.req.header("origin")),
      resolveApiOrigin(c.req.url, c.req.raw.headers),
    ),
  );
});

github.post("/webhook", async (c) => {
  const event = c.req.header("x-github-event") ?? "";
  const delivery = c.req.header("x-github-delivery") ?? "";
  const appId = c.req.header("x-github-hook-installation-target-id") ?? "";
  const signature = c.req.header("x-hub-signature-256") ?? "";
  const rawBody = await c.req.text();

  if (event === "ping") return c.json({ ok: true });
  if (!appId) return c.json({ error: "Missing GitHub App id" }, 400);

  const integration = await getIntegrationByAppId(appId);
  if (!integration?.webhookSecret) return c.json({ error: "GitHub webhook not configured" }, 404);

  const webhookSecret = await decryptSecret(integration.webhookSecret);
  if (!verifyWebhookSignature(rawBody, signature, webhookSecret)) {
    return c.json({ error: "Invalid GitHub webhook signature" }, 401);
  }

  if (delivery) {
    const [createdDelivery] = await db
      .insert(githubWebhookDelivery)
      .values({
        id: crypto.randomUUID(),
        integrationId: integration.id,
        deliveryId: delivery,
        event,
        createdAt: new Date(),
      })
      .onConflictDoNothing({
        target: [githubWebhookDelivery.integrationId, githubWebhookDelivery.deliveryId],
      })
      .returning({ id: githubWebhookDelivery.id });

    if (!createdDelivery) return c.json({ ok: true, delivery, duplicate: true });
  }

  let payload: GitHubWebhookPayload;
  try {
    payload = JSON.parse(rawBody) as GitHubWebhookPayload;
  } catch {
    return c.json({ error: "Invalid GitHub webhook payload" }, 400);
  }

  if (event === "installation") {
    const installationId =
      payload.installation?.id === undefined ? "" : String(payload.installation.id);
    if (payload.action === "deleted" && installationId) {
      await db
        .delete(githubAppInstallation)
        .where(
          and(
            eq(githubAppInstallation.integrationId, integration.id),
            eq(githubAppInstallation.installationId, installationId),
          ),
        );
      return c.json({ ok: true, delivery, installationId, removed: true });
    }

    return c.json({
      ok: true,
      delivery,
      ignored: `installation:${payload.action ?? "unknown"}`,
    });
  }

  if (event !== "push") return c.json({ ok: true, delivery, ignored: event || "unknown" });

  const target = gitHubPushTarget(payload);
  if (!target) return c.json({ ok: true, ignored: "non-branch-push" });

  const rows = await db
    .select({
      id: appTable.id,
      repositoryUrl: appTable.repositoryUrl,
      autoRedeployEnabled: appTable.autoRedeployEnabled,
    })
    .from(appTable)
    .innerJoin(environment, eq(appTable.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(
      and(
        eq(project.organizationId, integration.organizationId),
        eq(appTable.sourceType, "repository"),
        eq(appTable.branch, target.branch),
      ),
    );

  const matched = rows.filter(
    (row) => gitHubRepositoryFullName(row.repositoryUrl) === target.fullName,
  );
  const deployable = matched.filter((row) => row.autoRedeployEnabled);
  const queued: string[] = [];
  const errors: { appId: string; error: string }[] = [];

  for (const row of deployable) {
    const result = await enqueueDeploy(row.id);
    if ("error" in result) {
      errors.push({ appId: row.id, error: result.error });
    } else {
      queued.push(result.deployment.id);
    }
  }

  return c.json({
    ok: true,
    delivery,
    matched: matched.length,
    skippedAutoRedeployDisabled: matched.length - deployable.length,
    queued,
    errors,
  });
});

github.get("/installations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const rows = await db
    .select()
    .from(githubAppInstallation)
    .where(eq(githubAppInstallation.organizationId, organizationId))
    .orderBy(githubAppInstallation.createdAt);

  return c.json(rows.map(toInstallation));
});

github.post("/installations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const integration = await getIntegration(organizationId);
  if (!integration) return c.json({ error: "Create a GitHub App first" }, 400);

  const body = (await c.req
    .json()
    .catch(() => null)) as Partial<SaveGitHubAppInstallationInput> | null;
  const installationId =
    typeof body?.installationId === "string" && body.installationId.trim()
      ? body.installationId.trim()
      : "";
  if (!/^\d+$/.test(installationId)) {
    return c.json({ error: "installationId is required" }, 400);
  }

  const jwt = await createAppJwt(integration);
  const remoteResult = await tryGitHub(() =>
    githubRequest<GitHubInstallationResponse>(
      "GET",
      `/app/installations/${installationId}`,
      undefined,
      jwt,
    ),
  );
  if (!remoteResult.ok) return c.json({ error: remoteResult.error }, 502);

  const row = await saveRemoteInstallation(organizationId, integration, remoteResult.value);
  if (!row) return c.json({ error: "GitHub installation has no account" }, 400);
  return c.json(toInstallation(row), 201);
});

github.post("/installations/sync", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const integration = await getIntegration(organizationId);
  if (!integration) return c.json({ error: "Create a GitHub App first" }, 400);

  const remoteResult = await tryGitHub(() => listAppInstallations(integration));
  if (!remoteResult.ok) return c.json({ error: remoteResult.error }, 502);

  const rows: GitHubInstallationRow[] = [];
  for (const remote of remoteResult.value) {
    const row = await saveRemoteInstallation(organizationId, integration, remote);
    if (row) rows.push(row);
  }

  return c.json(rows.map(toInstallation));
});

github.delete("/installations/:id", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  await db
    .delete(githubAppInstallation)
    .where(
      and(
        eq(githubAppInstallation.id, c.req.param("id")),
        eq(githubAppInstallation.organizationId, organizationId),
      ),
    );

  return c.body(null, 204);
});

github.get("/repositories", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  const repositoriesResult = await tryGitHub(() => listInstalledRepositories(organizationId));
  if (!repositoriesResult.ok) return c.json({ error: repositoriesResult.error }, 502);

  return c.json(repositoriesResult.value);
});

github.delete("/integration", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw);
  if (organizationId instanceof Response) return organizationId;

  await db
    .delete(githubAppIntegration)
    .where(eq(githubAppIntegration.organizationId, organizationId));

  return c.body(null, 204);
});

export async function resolveGitHubCloneToken(
  organizationId: string,
  repositoryUrl: string,
): Promise<string | null> {
  const owner = parseGitHubOwner(repositoryUrl);
  if (!owner) return null;

  const [row] = await db
    .select({
      integration: githubAppIntegration,
      installation: githubAppInstallation,
    })
    .from(githubAppInstallation)
    .innerJoin(
      githubAppIntegration,
      eq(githubAppInstallation.integrationId, githubAppIntegration.id),
    )
    .where(
      and(
        eq(githubAppInstallation.organizationId, organizationId),
        sql`lower(${githubAppInstallation.accountLogin}) = ${owner.toLowerCase()}`,
      ),
    )
    .limit(1);

  if (!row) return null;

  const jwt = await createAppJwt(row.integration);
  return createInstallationToken(row.installation.installationId, jwt);
}

function createManifest(
  organizationId: string,
  requestUrl: string,
  headerOrigin?: string,
  headers?: Headers,
): GitHubAppManifest {
  const origin = resolveRequestOrigin(requestUrl, headerOrigin);
  const apiOrigin = resolveApiOrigin(requestUrl, headers);
  const state = createGitHubManifestState(organizationId);
  return {
    actionUrl: `${GITHUB_APP_CREATE_URL}?state=${encodeURIComponent(state)}`,
    manifest: JSON.stringify(createGitHubAppManifest(origin, apiOrigin)),
    state,
    webhookUrl: gitHubWebhookUrl(origin, apiOrigin),
  };
}

function resolveRequestOrigin(requestUrl: string, headerOrigin?: string): string {
  const url = new URL(requestUrl);
  return resolveManifestOrigin(requestUrl, headerOrigin, url.searchParams.get("origin"));
}

async function getIntegration(organizationId: string): Promise<GitHubIntegrationRow | null> {
  const [row] = await db
    .select()
    .from(githubAppIntegration)
    .where(eq(githubAppIntegration.organizationId, organizationId))
    .limit(1);
  return row ?? null;
}

async function getIntegrationByAppId(appId: string): Promise<GitHubIntegrationRow | null> {
  const [row] = await db
    .select()
    .from(githubAppIntegration)
    .where(eq(githubAppIntegration.appId, appId))
    .limit(1);
  return row ?? null;
}

function toIntegration(
  row: GitHubIntegrationRow | null,
  origin?: string,
  webhookOrigin?: string,
): GitHubAppIntegration {
  if (!row) {
    return {
      connected: false,
      webhookUrl: origin ? gitHubWebhookUrl(origin, webhookOrigin) : undefined,
    };
  }
  return {
    connected: true,
    appName: row.name,
    appSlug: row.slug,
    appId: row.appId,
    installUrl: `https://github.com/apps/${row.slug}/installations/select_target`,
    webhookUrl: origin ? gitHubWebhookUrl(origin, webhookOrigin) : undefined,
    updatedAt: row.updatedAt.toISOString(),
  };
}

function toInstallation(row: GitHubInstallationRow): GitHubAppInstallation {
  return {
    id: row.id,
    installationId: row.installationId,
    accountLogin: row.accountLogin,
    accountType: row.accountType,
    repositorySelection: row.repositorySelection,
    createdAt: row.createdAt.toISOString(),
    updatedAt: row.updatedAt.toISOString(),
  };
}

async function saveRemoteInstallation(
  organizationId: string,
  integration: GitHubIntegrationRow,
  remote: GitHubInstallationResponse,
): Promise<GitHubInstallationRow | null> {
  if (!remote.account?.login) return null;

  const now = new Date();
  const [row] = await db
    .insert(githubAppInstallation)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      integrationId: integration.id,
      installationId: String(remote.id),
      accountLogin: remote.account.login,
      accountType: remote.account.type ?? null,
      repositorySelection: remote.repository_selection ?? null,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: [githubAppInstallation.integrationId, githubAppInstallation.installationId],
      set: {
        accountLogin: remote.account.login,
        accountType: remote.account.type ?? null,
        repositorySelection: remote.repository_selection ?? null,
        updatedAt: now,
      },
    })
    .returning();

  return row ?? null;
}

async function listAppInstallations(
  integration: GitHubIntegrationRow,
): Promise<GitHubInstallationResponse[]> {
  const jwt = await createAppJwt(integration);
  const installations: GitHubInstallationResponse[] = [];
  let page = 1;

  while (page) {
    const parsed = await githubRequest<GitHubInstallationResponse[]>(
      "GET",
      `/app/installations?per_page=100&page=${page}`,
      undefined,
      jwt,
    );
    installations.push(...parsed);
    page = parsed.length === 100 ? page + 1 : 0;
  }

  return installations;
}

async function listInstalledRepositories(organizationId: string): Promise<GitHubRepositoryList> {
  const rows = await db
    .select({
      integration: githubAppIntegration,
      installation: githubAppInstallation,
    })
    .from(githubAppInstallation)
    .innerJoin(
      githubAppIntegration,
      eq(githubAppInstallation.integrationId, githubAppIntegration.id),
    )
    .where(eq(githubAppInstallation.organizationId, organizationId))
    .orderBy(githubAppInstallation.accountLogin);

  const repositories: GitHubRepository[] = [];
  const errors: string[] = [];

  for (const row of rows) {
    try {
      const jwt = await createAppJwt(row.integration);
      const token = await createInstallationToken(row.installation.installationId, jwt);
      let page = 1;
      while (page) {
        const parsed = await githubRequest<GitHubRepositoryResponse>(
          "GET",
          `/installation/repositories?per_page=100&page=${page}`,
          undefined,
          token,
        );
        repositories.push(
          ...parsed.repositories.map((repository) => ({
            id: String(repository.id),
            installationId: row.installation.installationId,
            accountLogin: row.installation.accountLogin,
            fullName: repository.full_name,
            cloneUrl: repository.clone_url,
            defaultBranch: repository.default_branch ?? "main",
            private: repository.private === true,
          })),
        );
        page = parsed.repositories.length === 100 ? page + 1 : 0;
      }
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      errors.push(`${row.installation.accountLogin}: ${message}`);
    }
  }

  if (repositories.length === 0 && errors.length > 0) {
    throw new Error(`Could not list GitHub repositories: ${errors.join("; ")}`);
  }

  return {
    repositories: repositories.sort((a, b) => a.fullName.localeCompare(b.fullName)),
    errors,
  };
}

async function createInstallationToken(installationId: string, jwt: string): Promise<string> {
  const token = await githubRequest<InstallationTokenResponse>(
    "POST",
    `/app/installations/${encodeURIComponent(installationId)}/access_tokens`,
    undefined,
    jwt,
  );
  return token.token;
}

async function githubRequest<T>(
  method: string,
  path: string,
  body?: unknown,
  bearer?: string,
): Promise<T> {
  const response = await fetch(`${GITHUB_API}${path}`, {
    method,
    headers: {
      accept: "application/vnd.github+json",
      "x-github-api-version": GITHUB_API_VERSION,
      ...(bearer ? { authorization: `Bearer ${bearer}` } : {}),
      ...(body ? { "content-type": "application/json" } : {}),
    },
    body: body ? JSON.stringify(body) : undefined,
  });

  const parsed = (await response.json().catch(() => null)) as GitHubError | T | null;
  if (!response.ok) {
    const message =
      parsed &&
      typeof parsed === "object" &&
      "message" in parsed &&
      typeof parsed.message === "string"
        ? parsed.message
        : `request failed with ${response.status}`;
    throw new Error(`GitHub: ${message}`);
  }

  return parsed as T;
}

async function tryGitHub<T>(
  run: () => Promise<T>,
): Promise<{ ok: true; value: T } | { ok: false; error: string }> {
  try {
    return { ok: true, value: await run() };
  } catch (error) {
    return {
      ok: false,
      error: error instanceof Error ? error.message : "GitHub request failed",
    };
  }
}

async function createAppJwt(integration: GitHubIntegrationRow): Promise<string> {
  const now = Math.floor(Date.now() / 1000);
  const header = base64UrlJson({ alg: "RS256", typ: "JWT" });
  const payload = base64UrlJson({
    iat: now - 60,
    exp: now + 540,
    iss: integration.appId,
  });
  const signingInput = `${header}.${payload}`;
  const signer = createSign("RSA-SHA256");
  signer.update(signingInput);
  signer.end();
  const signature = signer.sign(await decryptSecret(integration.privateKey));
  return `${signingInput}.${signature.toString("base64url")}`;
}

function base64UrlJson(value: unknown): string {
  return Buffer.from(JSON.stringify(value)).toString("base64url");
}
