import { db, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { enqueueAction } from "./queue";

/**
 * On API startup, re-enqueue every server still in 'provisioning' — its job was
 * interrupted by a restart/crash. With a durable queue this is the correct
 * recovery: re-enqueue is idempotent (jobId dedup collapses against any job that
 * is still live in Redis) and provisionServer itself is idempotent, so a healthy
 * in-flight provision is not disturbed and an interrupted one resumes.
 *
 * Never throws (Redis may be briefly unavailable at boot) so it cannot take down
 * the process.
 */
export async function reconcileProvisioningServers(): Promise<void> {
  const rows = await db
    .select({ id: server.id })
    .from(server)
    .where(eq(server.status, "provisioning"));

  for (const row of rows) {
    try {
      await enqueueAction("provision-server", row.id);
    } catch (error) {
      console.error("[reconcile] failed to re-enqueue", row.id, error);
    }
  }
}
