import { app, db, deployment, environment, project } from "@basse/db";
import type { DeploymentStatus } from "@basse/shared";
import { eq } from "drizzle-orm";
import { sendDeploymentEmail } from "./email";

type DeploymentNotificationStatus = Extract<DeploymentStatus, "healthy" | "failed">;

type DeploymentNotificationContext = {
  deployment: typeof deployment.$inferSelect;
  app: typeof app.$inferSelect;
  environment: typeof environment.$inferSelect;
  project: typeof project.$inferSelect;
};

function isDeploymentNotificationStatus(
  status: DeploymentStatus,
): status is DeploymentNotificationStatus {
  return status === "healthy" || status === "failed";
}

async function loadContext(
  deploymentId: string,
): Promise<DeploymentNotificationContext | null> {
  const [row] = await db
    .select({ deployment, app, environment, project })
    .from(deployment)
    .innerJoin(app, eq(deployment.appId, app.id))
    .innerJoin(environment, eq(app.environmentId, environment.id))
    .innerJoin(project, eq(environment.projectId, project.id))
    .where(eq(deployment.id, deploymentId))
    .limit(1);

  return row ?? null;
}

function webhookPayload(ctx: DeploymentNotificationContext, status: DeploymentNotificationStatus) {
  return {
    event: `deployment.${status === "healthy" ? "succeeded" : "failed"}`,
    deployment: {
      id: ctx.deployment.id,
      appId: ctx.deployment.appId,
      status,
      phase: ctx.deployment.phase,
      commitSha: ctx.deployment.commitSha,
      imageRef: ctx.deployment.imageRef,
      buildId: ctx.deployment.buildId,
      createdAt: ctx.deployment.createdAt.toISOString(),
      updatedAt: ctx.deployment.updatedAt.toISOString(),
    },
    app: {
      id: ctx.app.id,
      name: ctx.app.name,
      slug: ctx.app.slug,
    },
    environment: {
      id: ctx.environment.id,
      name: ctx.environment.name,
      slug: ctx.environment.slug,
    },
    project: {
      id: ctx.project.id,
      name: ctx.project.name,
      slug: ctx.project.slug,
    },
    organizationId: ctx.project.organizationId,
  };
}

async function sendWebhook(
  ctx: DeploymentNotificationContext,
  status: DeploymentNotificationStatus,
): Promise<void> {
  if (!ctx.app.deployWebhookUrl) return;

  const payload = webhookPayload(ctx, status);
  const body = JSON.stringify(payload);
  const deliveryId = crypto.randomUUID();
  const response = await fetch(ctx.app.deployWebhookUrl, {
    method: "POST",
    headers: {
      "content-type": "application/json",
      "user-agent": "Basse-Deploy-Webhook/1.0",
      "x-basse-event": payload.event,
      "x-basse-delivery": deliveryId,
      "x-basse-deployment-id": ctx.deployment.id,
    },
    body,
  });

  if (!response.ok) {
    const text = await response.text().catch(() => "");
    throw new Error(`webhook returned ${response.status}${text ? `: ${text.slice(0, 200)}` : ""}`);
  }
}

export async function notifyDeploymentFinished(
  deploymentId: string,
  status: DeploymentStatus,
): Promise<void> {
  if (!isDeploymentNotificationStatus(status)) return;

  const ctx = await loadContext(deploymentId);
  if (!ctx) return;

  const notifyEmail =
    status === "healthy" ? ctx.app.deployNotifySuccess : ctx.app.deployNotifyFailure;
  if (!ctx.app.deployWebhookUrl && !notifyEmail) return;

  await Promise.all([
    sendWebhook(ctx, status).catch((error) => {
      console.error(
        "[deploy-notify] webhook delivery failed",
        error instanceof Error ? error.message : error,
      );
    }),
    notifyEmail
      ? sendDeploymentEmail({
          id: ctx.deployment.id,
          appId: ctx.app.id,
          organizationId: ctx.project.organizationId,
          appName: ctx.app.name,
          projectName: ctx.project.name,
          environmentName: ctx.environment.name,
          status,
          commitSha: ctx.deployment.commitSha,
        })
      : Promise.resolve(),
  ]);
}
