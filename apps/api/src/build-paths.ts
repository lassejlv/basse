import { existsSync, statSync } from "node:fs";
import { isAbsolute, relative, resolve } from "node:path";
import { posix } from "node:path";

export const DEFAULT_BUILD_ROOT_DIRECTORY = "";
export const DEFAULT_DOCKERFILE_PATH = "Dockerfile";

export function normalizeBuildRootDirectory(value?: string | null): string {
  return normalizeRepoRelativePath(value, DEFAULT_BUILD_ROOT_DIRECTORY, "build root directory");
}

export function normalizeDockerfilePath(value?: string | null): string {
  return normalizeRepoRelativePath(value, DEFAULT_DOCKERFILE_PATH, "Dockerfile path");
}

export function resolveBuildPaths(
  repoDir: string,
  buildRootDirectory?: string | null,
  dockerfilePath?: string | null,
): { buildDir: string; dockerfilePath: string; dockerfilePathRelative: string } {
  const normalizedRoot = normalizeBuildRootDirectory(buildRootDirectory);
  const normalizedDockerfile = normalizeDockerfilePath(dockerfilePath);
  const buildDir = normalizedRoot ? resolve(repoDir, normalizedRoot) : repoDir;
  assertInside(repoDir, buildDir, "build root directory");
  if (!existsSync(buildDir) || !statSync(buildDir).isDirectory()) {
    throw new Error(`Build root directory not found: ${normalizedRoot || "."}`);
  }

  const dockerfile = resolve(buildDir, normalizedDockerfile);
  assertInside(buildDir, dockerfile, "Dockerfile path");
  return {
    buildDir,
    dockerfilePath: dockerfile,
    dockerfilePathRelative: normalizedDockerfile,
  };
}

function normalizeRepoRelativePath(
  value: string | null | undefined,
  defaultValue: string,
  label: string,
): string {
  const raw = (value ?? "").trim();
  if (!raw || raw === ".") return defaultValue;
  if (raw.includes("\0")) throw new Error(`${label} must be a relative path`);
  if (raw.includes("\\")) throw new Error(`${label} must use forward slashes`);
  if (raw.startsWith("/")) throw new Error(`${label} must be relative`);

  const normalized = posix.normalize(raw).replace(/^\.\//, "");
  if (!normalized || normalized === ".") return defaultValue;
  if (normalized === ".." || normalized.startsWith("../")) {
    throw new Error(`${label} cannot escape the repository`);
  }
  if (normalized.split("/").some((part) => part === "..")) {
    throw new Error(`${label} cannot escape the repository`);
  }
  return normalized;
}

function assertInside(baseDir: string, candidate: string, label: string): void {
  const rel = relative(baseDir, candidate);
  if (rel && (rel.startsWith("..") || isAbsolute(rel))) {
    throw new Error(`${label} cannot escape the repository`);
  }
}
