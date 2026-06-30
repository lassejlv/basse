import { existsSync } from "node:fs";
import { join } from "node:path";

// Build orchestration: clone a public repo, detect Dockerfile vs Railpack, build
// REMOTELY on Depot, and mint a short-lived pull token. All commands run via
// Bun.spawn with argv (never a shell string) so user-controlled values can't
// inject. Depot builds remotely, so the control plane only needs git + the depot
// and railpack CLIs (installed in the Docker image).

const RAILPACK_FRONTEND =
  Bun.env.BASSE_RAILPACK_FRONTEND ?? "ghcr.io/railwayapp/railpack-frontend:0.30.0";

export type BuildLogger = (line: string) => void;

type SpawnResult = { exitCode: number; output: string };

async function run(
  cmd: string[],
  options: { cwd?: string; env?: Record<string, string>; onLine?: BuildLogger; timeoutMs?: number },
): Promise<SpawnResult> {
  const proc = Bun.spawn(cmd, {
    cwd: options.cwd,
    env: { ...process.env, ...options.env } as Record<string, string>,
    stdout: "pipe",
    stderr: "pipe",
    signal: AbortSignal.timeout(options.timeoutMs ?? 600_000),
  });

  let output = "";
  await Promise.all(
    [proc.stdout, proc.stderr].map(async (stream) => {
      const decoder = new TextDecoder();
      let buffer = "";
      for await (const chunk of stream) {
        const text = decoder.decode(chunk, { stream: true });
        output += text;
        buffer += text;
        let nl = buffer.indexOf("\n");
        while (nl !== -1) {
          options.onLine?.(buffer.slice(0, nl));
          buffer = buffer.slice(nl + 1);
          nl = buffer.indexOf("\n");
        }
      }
      if (buffer && options.onLine) options.onLine(buffer);
    }),
  );

  const exitCode = await proc.exited;
  return { exitCode, output };
}

const BRANCH_PATTERN = /^[A-Za-z0-9._/-]+$/;

/**
 * Shallow-clones a public repo at a branch into ctxDir. No credentials are ever
 * supplied (GIT_TERMINAL_PROMPT/ASKPASS disabled), so a private/auth repo fails
 * fast rather than hanging. Returns the commit SHA.
 */
export async function cloneRepo(opts: {
  repositoryUrl: string;
  branch: string;
  ctxDir: string;
  onLine?: BuildLogger;
}): Promise<string> {
  if (!BRANCH_PATTERN.test(opts.branch) || opts.branch.startsWith("-")) {
    throw new Error(`invalid branch: ${opts.branch}`);
  }

  const cloneEnv = { GIT_TERMINAL_PROMPT: "0", GIT_ASKPASS: "/bin/true" };
  const clone = await run(
    ["git", "clone", "--depth", "1", "--branch", opts.branch, "--", opts.repositoryUrl, opts.ctxDir],
    { env: cloneEnv, onLine: opts.onLine, timeoutMs: 120_000 },
  );
  if (clone.exitCode !== 0) {
    throw new Error(`git clone failed: ${clone.output.trim().split("\n").slice(-2).join(" ")}`);
  }

  const rev = await run(["git", "-C", opts.ctxDir, "rev-parse", "HEAD"], { timeoutMs: 15_000 });
  return rev.output.trim();
}

/** Resolves the effective build kind given the app's setting and the repo. */
export function resolveBuildKind(
  buildMode: "auto" | "dockerfile" | "railpack",
  ctxDir: string,
): "dockerfile" | "railpack" {
  if (buildMode === "dockerfile") return "dockerfile";
  if (buildMode === "railpack") return "railpack";
  return existsSync(join(ctxDir, "Dockerfile")) ? "dockerfile" : "railpack";
}

/**
 * Builds the context remotely on Depot and saves it to the ephemeral registry
 * tagged with the deployment id. Returns the Depot build id. For the no-Dockerfile
 * path, Railpack generates a plan and Depot's BuildKit runs the Railpack frontend
 * via BUILDKIT_SYNTAX.
 */
export async function buildImage(opts: {
  kind: "dockerfile" | "railpack";
  ctxDir: string;
  depotToken: string;
  projectId: string;
  deploymentId: string;
  metadataFile: string;
  onLine?: BuildLogger;
}): Promise<{ buildId: string | null }> {
  const env = { DEPOT_TOKEN: opts.depotToken };

  if (opts.kind === "railpack") {
    const planPath = join(opts.ctxDir, "railpack-plan.json");
    const prepare = await run(["railpack", "prepare", opts.ctxDir, "--plan-out", planPath], {
      env,
      onLine: opts.onLine,
      timeoutMs: 120_000,
    });
    if (prepare.exitCode !== 0) {
      throw new Error("railpack could not detect how to build this repo");
    }
  }

  const dockerfileArg =
    opts.kind === "dockerfile"
      ? ["-f", join(opts.ctxDir, "Dockerfile")]
      : ["--build-arg", `BUILDKIT_SYNTAX=${RAILPACK_FRONTEND}`, "-f", join(opts.ctxDir, "railpack-plan.json")];

  const build = await run(
    [
      "depot",
      "build",
      "--project",
      opts.projectId,
      "--save",
      "--save-tag",
      opts.deploymentId,
      "--metadata-file",
      opts.metadataFile,
      "--platform",
      "linux/amd64",
      "--progress",
      "plain",
      ...dockerfileArg,
      opts.ctxDir,
    ],
    { env, onLine: opts.onLine, timeoutMs: 1_200_000 },
  );

  if (build.exitCode !== 0) {
    throw new Error("build failed");
  }

  // The metadata file holds depot.build.buildID — for records / `depot pull`.
  let buildId: string | null = null;
  try {
    const meta = (await Bun.file(opts.metadataFile).json()) as Record<string, unknown>;
    const id = meta["depot.build.buildID"];
    if (typeof id === "string") buildId = id;
  } catch {
    buildId = null;
  }

  return { buildId };
}

/** Mints a short-lived (1h), read-only pull token for the Depot project. */
export async function mintPullToken(depotToken: string, projectId: string): Promise<string> {
  const result = await run(["depot", "pull-token", "--project", projectId], {
    env: { DEPOT_TOKEN: depotToken },
    timeoutMs: 30_000,
  });
  if (result.exitCode !== 0) {
    throw new Error("could not mint a Depot pull token");
  }
  // The CLI prints the token (last non-empty line).
  const token = result.output.trim().split("\n").filter(Boolean).pop() ?? "";
  if (!token) {
    throw new Error("empty pull token");
  }
  return token;
}
