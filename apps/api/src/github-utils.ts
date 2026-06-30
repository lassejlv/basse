export function parseGitHubOwner(repositoryUrl: string): string | null {
  const repository = parseGitHubRepository(repositoryUrl);
  return repository?.owner ?? null;
}

export function gitHubHttpsCloneUrl(repositoryUrl: string): string | null {
  const repository = parseGitHubRepository(repositoryUrl);
  if (!repository) return null;
  return `https://github.com/${repository.owner}/${repository.repo}.git`;
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

function normalizeRepository(owner?: string, repo?: string): { owner: string; repo: string } | null {
  if (!owner || !repo) return null;
  const normalizedRepo = repo.replace(/\.git$/, "");
  return normalizedRepo ? { owner, repo: normalizedRepo } : null;
}

export function resolveManifestOrigin(
  requestUrl: string,
  headerOrigin?: string,
  requestedOrigin?: string | null,
): string {
  if (Bun.env.WEB_ORIGIN) return Bun.env.WEB_ORIGIN;

  const url = new URL(requestUrl);
  const fallback = `${url.protocol}//${url.host}`;
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

  return safeOrigin ?? fallback;
}

export function createGitHubAppManifest(origin: string): Record<string, unknown> {
  const setupUrl = `${origin}/secrets`;
  const appName = Bun.env.GITHUB_APP_NAME ?? "Basse";

  return {
    name: `${appName} ${crypto.randomUUID().slice(0, 8)}`,
    url: origin,
    redirect_url: setupUrl,
    callback_urls: [setupUrl],
    setup_url: setupUrl,
    public: false,
    default_permissions: {
      contents: "read",
      metadata: "read",
    },
    default_events: [],
    setup_on_update: true,
  };
}
