import {
  db,
  githubAppInstallation,
  githubAppIntegration,
} from "@basse/db";
import type {
  CompleteGitHubAppManifestInput,
  GitHubAppInstallation,
  GitHubAppIntegration,
  GitHubAppManifest,
  GitHubRepository,
  SaveGitHubAppInstallationInput,
} from "@basse/shared";
import { and, eq, sql } from "drizzle-orm";
import { Hono } from "hono";
import { createSign } from "node:crypto";
import { decryptSecret, encryptSecret } from "./crypto";
import { createGitHubAppManifest, parseGitHubOwner, resolveManifestOrigin } from "./github-utils";
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

export const github = new Hono();

github.get("/integration", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const integration = await getIntegration(organizationId);
  return c.json(toIntegration(integration));
});

github.get("/manifest", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  return c.json(createManifest(c.req.url, c.req.header("origin")));
});

github.post("/manifest/complete", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const body = (await c.req.json().catch(() => null)) as
    | Partial<CompleteGitHubAppManifestInput>
    | null;
  const code = typeof body?.code === "string" ? body.code.trim() : "";
  if (!code) return c.json({ error: "code is required" }, 400);

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
          webhookSecret: null,
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
        webhookSecret: null,
        createdAt: now,
        updatedAt: now,
      })
      .returning();
    return created;
  });

  if (!row) return c.json({ error: "Failed to save GitHub App" }, 500);
  return c.json(toIntegration(row));
});

github.get("/installations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const rows = await db
    .select()
    .from(githubAppInstallation)
    .where(eq(githubAppInstallation.organizationId, organizationId))
    .orderBy(githubAppInstallation.createdAt);

  return c.json(rows.map(toInstallation));
});

github.post("/installations", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const integration = await getIntegration(organizationId);
  if (!integration) return c.json({ error: "Create a GitHub App first" }, 400);

  const body = (await c.req.json().catch(() => null)) as
    | Partial<SaveGitHubAppInstallationInput>
    | null;
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

  const remote = remoteResult.value;
  if (!remote.account?.login) return c.json({ error: "GitHub installation has no account" }, 400);

  const now = new Date();
  const [row] = await db
    .insert(githubAppInstallation)
    .values({
      id: crypto.randomUUID(),
      organizationId,
      integrationId: integration.id,
      installationId: String(installationId),
      accountLogin: remote.account.login,
      accountType: remote.account.type ?? null,
      repositorySelection: remote.repository_selection ?? null,
      createdAt: now,
      updatedAt: now,
    })
    .onConflictDoUpdate({
      target: [
        githubAppInstallation.integrationId,
        githubAppInstallation.installationId,
      ],
      set: {
        accountLogin: remote.account.login,
        accountType: remote.account.type ?? null,
        repositorySelection: remote.repository_selection ?? null,
        updatedAt: now,
      },
    })
    .returning();

  if (!row) return c.json({ error: "Failed to save GitHub installation" }, 500);
  return c.json(toInstallation(row), 201);
});

github.get("/repositories", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  const repositoriesResult = await tryGitHub(() => listInstalledRepositories(organizationId));
  if (!repositoriesResult.ok) return c.json({ error: repositoriesResult.error }, 502);

  const repositories = repositoriesResult.value;
  return c.json(repositories);
});

github.delete("/integration", async (c) => {
  const organizationId = await resolveActiveWorkspace(c.req.raw.headers);
  if (organizationId instanceof Response) return organizationId;

  await db.delete(githubAppIntegration).where(eq(githubAppIntegration.organizationId, organizationId));

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

function createManifest(requestUrl: string, headerOrigin?: string): GitHubAppManifest {
  const url = new URL(requestUrl);
  const origin = resolveManifestOrigin(requestUrl, headerOrigin, url.searchParams.get("origin"));
  return {
    actionUrl: GITHUB_APP_CREATE_URL,
    manifest: JSON.stringify(createGitHubAppManifest(origin)),
  };
}

async function getIntegration(organizationId: string): Promise<GitHubIntegrationRow | null> {
  const [row] = await db
    .select()
    .from(githubAppIntegration)
    .where(eq(githubAppIntegration.organizationId, organizationId))
    .limit(1);
  return row ?? null;
}

function toIntegration(row: GitHubIntegrationRow | null): GitHubAppIntegration {
  if (!row) return { connected: false };
  return {
    connected: true,
    appName: row.name,
    appSlug: row.slug,
    appId: row.appId,
    installUrl: `https://github.com/apps/${row.slug}/installations/new`,
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

async function listInstalledRepositories(organizationId: string): Promise<GitHubRepository[]> {
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

  return repositories.sort((a, b) => a.fullName.localeCompare(b.fullName));
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
