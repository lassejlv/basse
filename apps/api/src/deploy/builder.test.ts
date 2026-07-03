import { mkdtemp, mkdir, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { describe, expect, test } from "bun:test";
import {
  normalizeBuildRootDirectory,
  normalizeDockerfilePath,
  resolveBuildPaths,
} from "./build-paths";
import { gitAskPassPath, gitAskPassScript, resolveBuildKind } from "./builder";

describe("gitAskPassScript", () => {
  test("reads the GitHub installation token from the environment", () => {
    const script = gitAskPassScript();
    expect(script).toContain("GIT_PASSWORD");
    expect(script).toContain("x-access-token");
  });

  test("does not embed a concrete token", () => {
    const token = "ghs_secret_installation_token";
    expect(gitAskPassScript()).not.toContain(token);
  });
});

describe("gitAskPassPath", () => {
  test("keeps the helper outside the clone destination", () => {
    const ctxDir = "/tmp/basse-build-abc123";
    const askPassPath = gitAskPassPath(ctxDir);
    expect(askPassPath).toBe("/tmp/basse-build-abc123.git-askpass.sh");
    expect(askPassPath.startsWith(`${ctxDir}/`)).toBe(false);
  });
});

describe("build paths", () => {
  test("normalizes default build paths", () => {
    expect(normalizeBuildRootDirectory("")).toBe("");
    expect(normalizeBuildRootDirectory(".")).toBe("");
    expect(normalizeDockerfilePath("")).toBe("Dockerfile");
    expect(normalizeDockerfilePath("./docker/Dockerfile.prod")).toBe("docker/Dockerfile.prod");
  });

  test("rejects paths that escape the repository", () => {
    expect(() => normalizeBuildRootDirectory("../app")).toThrow(
      "build root directory cannot escape the repository",
    );
    expect(() => normalizeDockerfilePath("/Dockerfile")).toThrow(
      "Dockerfile path must be relative",
    );
    expect(() => normalizeDockerfilePath("docker\\Dockerfile")).toThrow(
      "Dockerfile path must use forward slashes",
    );
  });

  test("auto detection uses the configured root and Dockerfile path", async () => {
    const repoDir = await mkdtemp(join(tmpdir(), "basse-build-paths-"));
    try {
      const buildDir = join(repoDir, "apps", "web");
      await mkdir(join(buildDir, "docker"), { recursive: true });
      await writeFile(join(buildDir, "docker", "Dockerfile.prod"), "FROM scratch\n");

      const paths = resolveBuildPaths(repoDir, "apps/web", "docker/Dockerfile.prod");

      expect(paths.buildDir).toBe(buildDir);
      expect(paths.dockerfilePathRelative).toBe("docker/Dockerfile.prod");
      expect(resolveBuildKind("auto", paths.buildDir, paths.dockerfilePath)).toBe("dockerfile");
      expect(resolveBuildKind("railpack", paths.buildDir, paths.dockerfilePath)).toBe("railpack");
    } finally {
      await rm(repoDir, { force: true, recursive: true });
    }
  });
});
