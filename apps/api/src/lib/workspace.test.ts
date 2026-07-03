import { describe, expect, test } from "bun:test";
import { apiTokenHasRequiredScope, requiredApiTokenScope } from "./workspace";

function request(method: string, path: string): Request {
  return new Request(`https://basse.example${path}`, { method });
}

describe("API token scopes", () => {
  test("requires read for GET requests", () => {
    const req = request("GET", "/api/apps");
    expect(requiredApiTokenScope(req)).toBe("read");
    expect(apiTokenHasRequiredScope(["read"], req)).toBe(true);
    expect(apiTokenHasRequiredScope(["deployments:write"], req)).toBe(false);
  });

  test("allows deployment automation without full write access", () => {
    const req = request("POST", "/api/deployments");
    expect(requiredApiTokenScope(req)).toBe("deployments:write");
    expect(apiTokenHasRequiredScope(["deployments:write"], req)).toBe(true);
    expect(apiTokenHasRequiredScope(["read"], req)).toBe(false);
  });

  test("requires write for other mutating routes", () => {
    const req = request("PATCH", "/api/apps/app_1");
    expect(requiredApiTokenScope(req)).toBe("write");
    expect(apiTokenHasRequiredScope(["write"], req)).toBe(true);
    expect(apiTokenHasRequiredScope(["deployments:write"], req)).toBe(false);
  });
});
