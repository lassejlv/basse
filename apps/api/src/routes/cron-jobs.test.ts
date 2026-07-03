import { describe, expect, test } from "bun:test";
import { cronMatches } from "../lib/cron-schedule";

describe("cronMatches", () => {
  const date = new Date(2026, 6, 2, 10, 15);

  test("supports wildcard steps", () => {
    expect(cronMatches("*/5 * * * *", date)).toBe(true);
    expect(cronMatches("*/7 * * * *", date)).toBe(false);
  });

  test("supports exact values, lists, and ranges", () => {
    expect(cronMatches("15 10 2 7 4", date)).toBe(true);
    expect(cronMatches("0,15,30 9-11 * * *", date)).toBe(true);
    expect(cronMatches("15 10 * * 7", date)).toBe(false);
  });

  test("rejects invalid expressions", () => {
    expect(cronMatches("61 * * * *", date)).toBe(false);
    expect(cronMatches("* * *", date)).toBe(false);
  });
});
