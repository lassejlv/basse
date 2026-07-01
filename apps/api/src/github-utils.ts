import { createHmac, timingSafeEqual } from "node:crypto";

const GITHUB_MANIFEST_STATE_TTL_MS = 30 * 60 * 1000;

export function parseGitHubOwner(repositoryUrl: string): string | null {
  const repository = parseGitHubRepository(repositoryUrl);
  return repository?.owner ?? null;
}

export function gitHubHttpsCloneUrl(repositoryUrl: string): string | null {
  const repository = parseGitHubRepository(repositoryUrl);
  if (!repository) return null;
  return `https://github.com/${repository.owner}/${repository.repo}.git`;
}

export function gitHubRepositoryFullName(repositoryUrl: string): string | null {
  const repository = parseGitHubRepository(repositoryUrl);
  if (!repository) return null;
  return `${repository.owner}/${repository.repo}`.toLowerCase();
}

export function gitHubPushTarget(payload: {
  ref?: string;
  deleted?: boolean;
  repository?: { full_name?: string };
}): { branch: string; fullName: string } | null {
  if (payload.deleted) return null;
  const branch = payload.ref?.startsWith("refs/heads/")
    ? payload.ref.slice("refs/heads/".length)
    : "";
  const fullName = payload.repository?.full_name?.toLowerCase() ?? "";
  return branch && fullName ? { branch, fullName } : null;
}

function parseGitHubRepository(repositoryUrl: string): { owner: string; repo: string } | null {
  try {
    const url = new URL(repositoryUrl);
    if (url.hostname !== "github.com") return null;
    if (url.protocol === "ssh:" && url.username && url.username !== "git") return null;
    const [owner, repo] = url.pathname.replace(/^\/+/, "").split("/");
    return normalizeRepository(owner, repo);
  } catch {
    const match = repositoryUrl.match(/^git@github\.com:([^/\s]+)\/([^/\s]+?)(?:\.git)?$/);
    if (!match) return null;
    return normalizeRepository(match[1], match[2]);
  }
}

function normalizeRepository(
  owner?: string,
  repo?: string,
): { owner: string; repo: string } | null {
  if (!owner || !repo) return null;
  const normalizedRepo = repo.replace(/\.git$/, "");
  return normalizedRepo ? { owner, repo: normalizedRepo } : null;
}

export function resolveManifestOrigin(
  requestUrl: string,
  headerOrigin?: string,
  requestedOrigin?: string | null,
): string {
  if (Bun.env.WEB_ORIGIN) return normalizeOrigin(Bun.env.WEB_ORIGIN);

  const url = new URL(requestUrl);
  const fallback = normalizeOrigin(`${url.protocol}//${url.host}`);
  const candidates = [requestedOrigin, headerOrigin].filter((value): value is string =>
    Boolean(value),
  );
  const safeOrigin = candidates.find((candidate) => {
    try {
      const parsed = new URL(candidate);
      return (
        (parsed.protocol === "http:" || parsed.protocol === "https:") &&
        ["localhost", "127.0.0.1", "::1"].includes(parsed.hostname)
      );
    } catch {
      return false;
    }
  });

  return safeOrigin ? normalizeOrigin(safeOrigin) : fallback;
}

function normalizeOrigin(value: string): string {
  const parsed = new URL(value);
  return parsed.origin;
}

export function resolveApiOrigin(requestUrl: string, headers?: Headers): string {
  if (Bun.env.API_ORIGIN) return normalizeOrigin(Bun.env.API_ORIGIN);

  const requestOrigin = normalizeOrigin(requestUrl);
  const forwardedHost = firstHeaderValue(headers?.get("x-forwarded-host"));
  const forwardedProto = firstHeaderValue(headers?.get("x-forwarded-proto"));
  const host = forwardedHost ?? firstHeaderValue(headers?.get("host"));
  const protocol = forwardedProto ?? new URL(requestUrl).protocol.replace(/:$/, "");

  if (!host || !/^[a-z][a-z0-9+.-]*$/i.test(protocol)) return requestOrigin;
  try {
    return normalizeOrigin(`${protocol}://${host}`);
  } catch {
    return requestOrigin;
  }
}

function firstHeaderValue(value?: string | null): string | null {
  const first = value?.split(",")[0]?.trim();
  return first || null;
}

export function createGitHubAppManifest(
  origin: string,
  webhookOrigin = resolveGitHubWebhookOrigin(origin),
): Record<string, unknown> {
  const setupUrl = `${origin}/secrets`;
  const appName = Bun.env.GITHUB_APP_NAME ?? "Basse";

  return {
    name: `${appName} ${crypto.randomUUID().slice(0, 8)}`,
    url: origin,
    redirect_url: setupUrl,
    callback_urls: [setupUrl],
    setup_url: setupUrl,
    public: false,
    hook_attributes: {
      active: true,
      url: `${webhookOrigin}/api/github/webhook`,
    },
    default_permissions: {
      contents: "read",
      metadata: "read",
    },
    // Only subscribable events belong here. GitHub delivers `installation`
    // (and other app-lifecycle events) automatically, and listing it makes the
    // manifest invalid ("Default events unsupported: installation").
    default_events: ["push"],
    setup_on_update: true,
  };
}

export function gitHubWebhookUrl(
  origin: string,
  webhookOrigin = resolveGitHubWebhookOrigin(origin),
): string {
  return `${webhookOrigin}/api/github/webhook`;
}

export function createGitHubManifestState(
  organizationId: string,
  now = new Date(),
): string {
  const payload = Buffer.from(
    JSON.stringify({
      organizationId,
      exp: now.getTime() + GITHUB_MANIFEST_STATE_TTL_MS,
      nonce: crypto.randomUUID(),
    }),
  ).toString("base64url");
  return `${payload}.${signGitHubManifestState(payload)}`;
}

export function verifyGitHubManifestState(
  state: string,
  organizationId: string,
  now = new Date(),
): boolean {
  const [payload, signature] = state.split(".");
  if (!payload || !signature) return false;
  if (!safeEqual(signature, signGitHubManifestState(payload))) return false;

  let parsed: { organizationId?: unknown; exp?: unknown };
  try {
    parsed = JSON.parse(Buffer.from(payload, "base64url").toString("utf8")) as {
      organizationId?: unknown;
      exp?: unknown;
    };
  } catch {
    return false;
  }

  return (
    parsed.organizationId === organizationId &&
    typeof parsed.exp === "number" &&
    parsed.exp >= now.getTime()
  );
}

function resolveGitHubWebhookOrigin(origin: string): string {
  return Bun.env.API_ORIGIN ? normalizeOrigin(Bun.env.API_ORIGIN) : origin;
}

function signGitHubManifestState(payload: string): string {
  const secret = Bun.env.BETTER_AUTH_SECRET;
  if (!secret) throw new Error("BETTER_AUTH_SECRET is required for GitHub App setup state");
  return createHmac("sha256", secret).update(payload).digest("base64url");
}

function safeEqual(value: string, expected: string): boolean {
  const valueBuffer = Buffer.from(value);
  const expectedBuffer = Buffer.from(expected);
  if (valueBuffer.length !== expectedBuffer.length) return false;
  return timingSafeEqual(valueBuffer, expectedBuffer);
}

export function verifyWebhookSignature(
  rawBody: string,
  signature: string,
  secret: string,
): boolean {
  if (!signature.startsWith("sha256=")) return false;
  const expected = `sha256=${createHmac("sha256", secret).update(rawBody).digest("hex")}`;
  return safeEqual(signature, expected);
}
