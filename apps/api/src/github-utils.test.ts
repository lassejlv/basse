import { afterEach, describe, expect, test } from "bun:test";
import {
  createGitHubAppManifest,
  gitHubHttpsCloneUrl,
  parseGitHubOwner,
  resolveManifestOrigin,
} from "./github-utils";

describe("parseGitHubOwner", () => {
  test("returns the owner for GitHub HTTPS repositories", () => {
    expect(parseGitHubOwner("https://github.com/lassejlv/basse")).toBe("lassejlv");
    expect(parseGitHubOwner("https://github.com/org/private-repo.git")).toBe("org");
  });

  test("returns the owner for GitHub SSH repositories", () => {
    expect(parseGitHubOwner("git@github.com:lassejlv/basse.git")).toBe("lassejlv");
    expect(parseGitHubOwner("git@github.com:org/private-repo")).toBe("org");
  });

  test("ignores non-GitHub and malformed repository URLs", () => {
    expect(parseGitHubOwner("https://gitlab.com/lassejlv/basse")).toBeNull();
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
  });

  test("ignores non-GitHub repositories", () => {
    expect(gitHubHttpsCloneUrl("git@gitlab.com:org/repo.git")).toBeNull();
  });
});

describe("createGitHubAppManifest", () => {
  test("requests only repository read permissions needed for private clones", () => {
    const manifest = createGitHubAppManifest("https://basse.example");
    expect(manifest).toMatchObject({
      url: "https://basse.example",
      redirect_url: "https://basse.example/secrets",
      callback_urls: ["https://basse.example/secrets"],
      setup_url: "https://basse.example/secrets",
      public: false,
      default_permissions: {
        contents: "read",
        metadata: "read",
      },
      default_events: [],
      setup_on_update: true,
    });
    expect(manifest).not.toHaveProperty("hook_attributes");
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

  test("allows localhost browser origins during development", () => {
    delete Bun.env.WEB_ORIGIN;
    expect(
      resolveManifestOrigin(
        "http://127.0.0.1:3000/api/github/manifest",
        "http://127.0.0.1:5173",
      ),
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
