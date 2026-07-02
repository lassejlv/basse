import { Queue } from "bullmq";
import { createConnection } from "./connection";

// The single durable queue for background actions. Each action is a job NAME
// dispatched to a typed handler (see actions.ts). New actions (e.g.
// "sync-domains", "deploy-app") are added as a union member + a handler entry +
// an enqueue call site — no new queue.
export type ActionName =
  | "provision-server"
  | "sync-domains"
  | "sync-app-load-balancers"
  | "deploy-app"
  | "cron-job"
  | "database-backup"
  | "database-backup-upload";

export const ACTIONS_QUEUE = "basse-actions";

// enableOfflineQueue:false makes add() reject fast when Redis is down (instead of
// buffering forever), so the HTTP enqueue path can fail fast and return 503.
export const actionsQueue = new Queue(ACTIONS_QUEUE, {
  connection: createConnection({ enableOfflineQueue: false }),
  defaultJobOptions: {
    attempts: 1,
    // Free the jobId the instant the job settles, so a later re-provision always
    // enqueues fresh rather than colliding with a retained completed job.
    removeOnComplete: true,
    removeOnFail: true,
  },
});

actionsQueue.on("error", (error) => {
  console.error("[queue]", error.message);
});

const ENQUEUE_TIMEOUT_MS = 5000;

/**
 * Enqueues a background action. Most job ids are namespaced as
 * `${name}__${entityId}` so a duplicate enqueue while one is queued/active is
 * collapsed. Domain syncs are intentionally unique: each enqueue follows a DB
 * route-set change and must get a chance to reconcile Caddy to the latest state.
 * syncServerDomains serializes per server, so unique jobs do not race each other.
 *
 * Rejects within ENQUEUE_TIMEOUT_MS if Redis is unreachable. enableOfflineQueue
 * does NOT cover the never-connected-at-boot case (BullMQ's add() awaits its own
 * connection-ready gate, which an infinite retryStrategy never satisfies), so we
 * bound it explicitly — the HTTP path can then fail fast and return 503.
 *
 * Note: BullMQ forbids ":" in a custom jobId (it is reserved for repeatable
 * jobs), so the separator is "__". The jobId is opaque — only used for dedup.
 */
export async function enqueueAction(name: ActionName, entityId: string): Promise<void> {
  const jobId =
    name === "sync-domains" || name === "sync-app-load-balancers"
      ? `${name}__${entityId}__${Date.now()}__${crypto.randomUUID()}`
      : `${name}__${entityId}`;
  const add = actionsQueue.add(name, { entityId }, { jobId });

  let timer: ReturnType<typeof setTimeout> | undefined;
  const timeout = new Promise<never>((_, reject) => {
    timer = setTimeout(
      () => reject(new Error("enqueue timed out (queue unavailable)")),
      ENQUEUE_TIMEOUT_MS,
    );
  });

  try {
    await Promise.race([add, timeout]);
  } finally {
    clearTimeout(timer);
  }
}
