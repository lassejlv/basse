import { type Job, Worker } from "bullmq";
import { actionHandlers } from "./actions";
import { createConnection } from "./connection";
import { ACTIONS_QUEUE, type ActionName } from "./queue";

type ActionJobData = { entityId: string };

/**
 * Starts the in-process worker that consumes background actions. Returns the
 * Worker handle so the caller can close() it on graceful shutdown (which waits
 * for the in-flight job to finish).
 *
 * lockDuration is 60s because provisioning is a multi-minute SSH job — the
 * default 30s lock would wrongly mark a healthy job stalled and re-run it. The
 * lock auto-renews as long as the event loop stays responsive, which it does
 * because handlers are I/O-bound (SSH/network), not CPU-bound.
 */
export function startWorker(): Worker<ActionJobData> {
  const worker = new Worker<ActionJobData>(
    ACTIONS_QUEUE,
    async (job: Job<ActionJobData>) => {
      const handler = actionHandlers[job.name as ActionName];

      if (!handler) {
        throw new Error(`Unknown action: ${job.name}`);
      }

      await handler(job.data.entityId);
    },
    {
      connection: createConnection(),
      concurrency: Number(Bun.env.QUEUE_CONCURRENCY ?? 5),
      lockDuration: 60_000,
      stalledInterval: 30_000,
      maxStalledCount: 1,
    },
  );

  worker.on("active", (job) => {
    console.log(`[worker] ${job.name} ${job.data.entityId} started`);
  });
  worker.on("completed", (job) => {
    console.log(`[worker] ${job.name} ${job.data.entityId} completed`);
  });
  worker.on("failed", (job, error) => {
    console.error(`[worker] ${job?.name} ${job?.data.entityId} failed: ${error.message}`);
  });
  worker.on("error", (error) => {
    console.error("[worker]", error.message);
  });

  return worker;
}
