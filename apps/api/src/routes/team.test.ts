import { describe, expect, test } from "bun:test";
import { canManageTeam } from "./team";

describe("team roles", () => {
  test("only owners and admins can manage team membership", () => {
    expect(canManageTeam("owner")).toBe(true);
    expect(canManageTeam("admin")).toBe(true);
    expect(canManageTeam("member")).toBe(false);
  });
});
