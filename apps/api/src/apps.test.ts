import { describe, expect, it } from "bun:test";
import type { app as appTable } from "@basse/db";

type AppRow = typeof appTable.$inferSelect;

const baseApp = {
  id: "app_1",
  environmentId: "env_1",
  serverId: null,
  name: "Gateway",
  slug: "gateway",
  repositoryUrl: "https://github.com/acme/gateway",
  branch: "main",
  port: 3000,
  buildMode: "auto",
  buildRootDirectory: "",
  dockerfilePath: "Dockerfile",
  buildRunner: "depot",
  autoRedeployEnabled: true,
  appKind: "service",
  sourceType: "repository",
  imageRef: null,
  volumes: "[]",
  cpuLimitMillicores: null,
  memoryLimitBytes: null,
  databaseKind: null,
  databaseVersion: null,
  databaseName: null,
  databaseUser: null,
  databasePassword: null,
  databasePublicEnabled: false,
  databasePublicPort: null,
  neonProjectId: null,
  neonRegion: null,
  neonConnectionUri: null,
  healthCheckEnabled: false,
  healthCheckPath: "/",
  healthCheckStatus: 200,
  healthCheckTimeoutSeconds: 5,
  healthCheckIntervalSeconds: 30,
  backupScheduleEnabled: false,
  backupIntervalHours: 24,
  backupRetention: 7,
  backupS3ConnectionId: null,
  deployWebhookUrl: null,
  deployNotifySuccess: false,
  deployNotifyFailure: false,
  createdAt: new Date("2026-01-01T00:00:00.000Z"),
  updatedAt: new Date("2026-01-01T00:00:00.000Z"),
} satisfies AppRow;

describe("buildAppUpdates", () => {
  it("normalizes app renames and keeps the slug in sync", async () => {
    Bun.env.BETTER_AUTH_SECRET ??= "test-secret-at-least-32-characters";
    const { buildAppUpdates } = await import("./apps");
    const result = await buildAppUpdates(baseApp, { name: "  Edge API  " }, "org_1");

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.updates.name).toBe("Edge API");
    expect(result.updates.slug).toBe("edge-api");
  });

  it("does not validate repositoryUrl when switching to a Docker image source", async () => {
    Bun.env.BETTER_AUTH_SECRET ??= "test-secret-at-least-32-characters";
    const { buildAppUpdates } = await import("./apps");
    const result = await buildAppUpdates(
      baseApp,
      {
        sourceType: "image",
        repositoryUrl: "",
        imageRef: "ghcr.io/drizzle-team/gateway",
        port: 3000,
      },
      "org_1",
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.updates.sourceType).toBe("image");
    expect(result.updates.imageRef).toBe("ghcr.io/drizzle-team/gateway");
    expect(result.updates.repositoryUrl).toBeUndefined();
  });

  it("normalizes deploy notification settings", async () => {
    Bun.env.BETTER_AUTH_SECRET ??= "test-secret-at-least-32-characters";
    const { buildAppUpdates } = await import("./apps");
    const result = await buildAppUpdates(
      baseApp,
      {
        deployWebhookUrl: " https://hooks.example.com/basse ",
        deployNotifySuccess: true,
        deployNotifyFailure: true,
      },
      "org_1",
    );

    expect(result.ok).toBe(true);
    if (!result.ok) return;
    expect(result.updates.deployWebhookUrl).toBe("https://hooks.example.com/basse");
    expect(result.updates.deployNotifySuccess).toBe(true);
    expect(result.updates.deployNotifyFailure).toBe(true);
  });

  it("rejects invalid deploy webhook URLs", async () => {
    Bun.env.BETTER_AUTH_SECRET ??= "test-secret-at-least-32-characters";
    const { buildAppUpdates } = await import("./apps");
    const result = await buildAppUpdates(
      baseApp,
      { deployWebhookUrl: "ftp://hooks.example.com/basse" },
      "org_1",
    );

    expect(result.ok).toBe(false);
    if (result.ok) return;
    expect(result.error).toBe("Webhook URL must use http or https");
  });
});
