import { describe, expect, test } from "bun:test";
import { gitAskPassPath, gitAskPassScript } from "./builder";

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
