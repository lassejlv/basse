import { afterEach, beforeEach, describe, expect, test } from "bun:test";
import {
  createGitHubAppManifest,
  createGitHubManifestState,
  gitHubHttpsCloneUrl,
  gitHubPushTarget,
  gitHubRepositoryFullName,
  gitHubWebhookUrl,
  parseGitHubOwner,
  resolveApiOrigin,
  resolveManifestOrigin,
  verifyGitHubManifestState,
} from "./github-utils";

describe("parseGitHubOwner", () => {
  test("returns the owner for GitHub HTTPS repositories", () => {
    expect(parseGitHubOwner("https://github.com/lassejlv/basse")).toBe("lassejlv");
    expect(parseGitHubOwner("https://github.com/org/private-repo.git")).toBe("org");
  });

  test("returns the owner for GitHub SSH repositories", () => {
    expect(parseGitHubOwner("git@github.com:lassejlv/basse.git")).toBe("lassejlv");
    expect(parseGitHubOwner("git@github.com:org/private-repo")).toBe("org");
    expect(parseGitHubOwner("ssh://git@github.com/lassejlv/basse.git")).toBe("lassejlv");
  });

  test("ignores non-GitHub and malformed repository URLs", () => {
    expect(parseGitHubOwner("https://gitlab.com/lassejlv/basse")).toBeNull();
    expect(parseGitHubOwner("ssh://deploy@github.com/lassejlv/basse.git")).toBeNull();
    expect(parseGitHubOwner("not a url")).toBeNull();
    expect(parseGitHubOwner("https://github.com/lassejlv")).toBeNull();
  });
});

describe("gitHubHttpsCloneUrl", () => {
  test("normalizes GitHub HTTPS and SSH repository URLs to HTTPS clone URLs", () => {
    expect(gitHubHttpsCloneUrl("https://github.com/lassejlv/basse")).toBe(
      "https://github.com/lassejlv/basse.git",
    );
    expect(gitHubHttpsCloneUrl("git@github.com:org/private-repo.git")).toBe(
      "https://github.com/org/private-repo.git",
    );
    expect(gitHubHttpsCloneUrl("ssh://git@github.com/org/private-repo.git")).toBe(
      "https://github.com/org/private-repo.git",
    );
    expect(gitHubHttpsCloneUrl("https://github.com/org/private-repo/tree/main")).toBe(
      "https://github.com/org/private-repo.git",
    );
  });

  test("ignores non-GitHub repositories", () => {
    expect(gitHubHttpsCloneUrl("git@gitlab.com:org/repo.git")).toBeNull();
  });
});

describe("gitHubRepositoryFullName", () => {
  test("normalizes GitHub repository references for matching webhook payloads", () => {
    expect(gitHubRepositoryFullName("https://github.com/LasseJlv/Basse/tree/main")).toBe(
      "lassejlv/basse",
    );
    expect(gitHubRepositoryFullName("git@github.com:Org/private-repo.git")).toBe(
      "org/private-repo",
    );
  });
});

describe("gitHubPushTarget", () => {
  test("extracts branch and repository full name from branch pushes", () => {
    expect(
      gitHubPushTarget({
        ref: "refs/heads/main",
        repository: { full_name: "LasseJlv/Basse" },
      }),
    ).toEqual({ branch: "main", fullName: "lassejlv/basse" });
  });

  test("ignores non-branch pushes and incomplete payloads", () => {
    expect(
      gitHubPushTarget({
        ref: "refs/tags/v1.0.0",
        repository: { full_name: "lassejlv/basse" },
      }),
    ).toBeNull();
    expect(gitHubPushTarget({ ref: "refs/heads/main" })).toBeNull();
  });

  test("ignores deleted branch pushes", () => {
    expect(
      gitHubPushTarget({
        deleted: true,
        ref: "refs/heads/main",
        repository: { full_name: "lassejlv/basse" },
      }),
    ).toBeNull();
  });
});

describe("createGitHubAppManifest", () => {
  const previousApiOrigin = Bun.env.API_ORIGIN;

  afterEach(() => {
    if (previousApiOrigin === undefined) {
      delete Bun.env.API_ORIGIN;
    } else {
      Bun.env.API_ORIGIN = previousApiOrigin;
    }
  });

  test("requests repository access and push webhooks needed for private deploys", () => {
    const manifest = createGitHubAppManifest("https://basse.example");
    expect(manifest).toMatchObject({
      url: "https://basse.example",
      redirect_url: "https://basse.example/secrets",
      callback_urls: ["https://basse.example/secrets"],
      setup_url: "https://basse.example/secrets",
      public: false,
      hook_attributes: {
        active: true,
        url: "https://basse.example/api/github/webhook",
      },
      default_permissions: {
        contents: "read",
        metadata: "read",
      },
      default_events: ["push", "installation"],
      setup_on_update: true,
    });
  });

  test("allows API_ORIGIN to host GitHub webhooks separately from the web UI", () => {
    Bun.env.API_ORIGIN = "https://api.basse.example/";
    expect(createGitHubAppManifest("https://app.basse.example")).toMatchObject({
      hook_attributes: {
        url: "https://api.basse.example/api/github/webhook",
      },
    });
    expect(gitHubWebhookUrl("https://app.basse.example")).toBe(
      "https://api.basse.example/api/github/webhook",
    );
  });

  test("supports explicit split origins for web callbacks and API webhooks", () => {
    expect(
      createGitHubAppManifest("https://app.basse.example", "https://api.basse.example"),
    ).toMatchObject({
      redirect_url: "https://app.basse.example/secrets",
      callback_urls: ["https://app.basse.example/secrets"],
      setup_url: "https://app.basse.example/secrets",
      hook_attributes: {
        url: "https://api.basse.example/api/github/webhook",
      },
    });
    expect(gitHubWebhookUrl("https://app.basse.example", "https://api.basse.example")).toBe(
      "https://api.basse.example/api/github/webhook",
    );
  });

  test("exposes the same webhook URL used by the manifest", () => {
    const origin = "https://basse.example";
    const manifest = createGitHubAppManifest(origin);
    expect(gitHubWebhookUrl(origin)).toBe("https://basse.example/api/github/webhook");
    expect(manifest).toMatchObject({
      hook_attributes: {
        url: gitHubWebhookUrl(origin),
      },
    });
  });
});

describe("resolveApiOrigin", () => {
  const previousApiOrigin = Bun.env.API_ORIGIN;

  afterEach(() => {
    if (previousApiOrigin === undefined) {
      delete Bun.env.API_ORIGIN;
    } else {
      Bun.env.API_ORIGIN = previousApiOrigin;
    }
  });

  test("prefers API_ORIGIN for GitHub webhooks", () => {
    Bun.env.API_ORIGIN = "https://api.basse.example/";
    expect(resolveApiOrigin("http://127.0.0.1:3000/api/github/manifest")).toBe(
      "https://api.basse.example",
    );
  });

  test("uses forwarded proxy headers before the internal request URL", () => {
    const headers = new Headers({
      "x-forwarded-host": "api.basse.example",
      "x-forwarded-proto": "https",
    });
    expect(resolveApiOrigin("http://127.0.0.1:3000/api/github/manifest", headers)).toBe(
      "https://api.basse.example",
    );
  });

  test("falls back to the request URL when proxy headers are malformed", () => {
    const headers = new Headers({
      "x-forwarded-host": "not a host/",
      "x-forwarded-proto": "https",
    });
    expect(resolveApiOrigin("http://127.0.0.1:3000/api/github/manifest", headers)).toBe(
      "http://127.0.0.1:3000",
    );
  });
});

describe("resolveManifestOrigin", () => {
  const previousWebOrigin = Bun.env.WEB_ORIGIN;

  afterEach(() => {
    if (previousWebOrigin === undefined) {
      delete Bun.env.WEB_ORIGIN;
    } else {
      Bun.env.WEB_ORIGIN = previousWebOrigin;
    }
  });

  test("prefers explicit WEB_ORIGIN in production", () => {
    Bun.env.WEB_ORIGIN = "https://basse.example";
    expect(resolveManifestOrigin("http://127.0.0.1:3000/api/github/manifest")).toBe(
      "https://basse.example",
    );
  });

  test("normalizes explicit WEB_ORIGIN before building callback URLs", () => {
    Bun.env.WEB_ORIGIN = "https://basse.example/";
    const origin = resolveManifestOrigin("http://127.0.0.1:3000/api/github/manifest");
    expect(origin).toBe("https://basse.example");
    expect(createGitHubAppManifest(origin)).toMatchObject({
      redirect_url: "https://basse.example/secrets",
      setup_url: "https://basse.example/secrets",
    });
  });

  test("allows localhost browser origins during development", () => {
    delete Bun.env.WEB_ORIGIN;
    expect(
      resolveManifestOrigin("http://127.0.0.1:3000/api/github/manifest", "http://127.0.0.1:5173"),
    ).toBe("http://127.0.0.1:5173");
  });

  test("rejects arbitrary origin query values", () => {
    delete Bun.env.WEB_ORIGIN;
    expect(
      resolveManifestOrigin(
        "http://127.0.0.1:3000/api/github/manifest",
        undefined,
        "https://evil.example",
      ),
    ).toBe("http://127.0.0.1:3000");
  });
});

describe("GitHub App setup state", () => {
  const previousSecret = Bun.env.BETTER_AUTH_SECRET;

  beforeEach(() => {
    Bun.env.BETTER_AUTH_SECRET = "github-state-test-secret";
  });

  afterEach(() => {
    if (previousSecret === undefined) {
      delete Bun.env.BETTER_AUTH_SECRET;
    } else {
      Bun.env.BETTER_AUTH_SECRET = previousSecret;
    }
  });

  test("accepts a signed state for the same workspace", () => {
    const now = new Date("2026-07-01T00:00:00.000Z");
    const state = createGitHubManifestState("workspace_1", now);

    expect(verifyGitHubManifestState(state, "workspace_1", now)).toBe(true);
  });

  test("rejects states for a different workspace", () => {
    const now = new Date("2026-07-01T00:00:00.000Z");
    const state = createGitHubManifestState("workspace_1", now);

    expect(verifyGitHubManifestState(state, "workspace_2", now)).toBe(false);
  });

  test("rejects expired and malformed states", () => {
    const now = new Date("2026-07-01T00:00:00.000Z");
    const state = createGitHubManifestState("workspace_1", now);

    expect(
      verifyGitHubManifestState(state, "workspace_1", new Date("2026-07-01T00:31:00.000Z")),
    ).toBe(false);
    expect(verifyGitHubManifestState("not-a-state", "workspace_1", now)).toBe(false);
  });
});
