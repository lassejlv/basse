import { runBackupUpload, runDatabaseBackup } from "../routes/backups";
import { runCronJob } from "../routes/cron-jobs";
import { runDeployment } from "../deploy/deploy";
import { syncManagedLoadBalancersForApp } from "../routes/load-balancers";
import { provisionServer } from "../infra/provision";
import { waitForDigitalOceanServer } from "../infra/digitalocean-server";
import { waitForHetznerServer } from "../infra/hetzner-server";
import { syncServerDomains } from "../deploy/proxy-sync";
import type { ActionName } from "./queue";

// Maps each action to its handler. Imported ONLY by worker.ts — never by the
// enqueue/route path — so provisionServer (and its ssh/agent imports) stay out of
// the HTTP request module graph. Each handler receives the entity id and must
// stay async/non-blocking (synchronous work longer than the worker lock duration
// would stall and re-run the job).
export const actionHandlers: Record<ActionName, (entityId: string) => Promise<void>> = {
  "provision-server": (serverId) => provisionServer(serverId),
  "digitalocean-wait-server": (serverId) => waitForDigitalOceanServer(serverId),
  "hetzner-wait-server": (serverId) => waitForHetznerServer(serverId),
  "sync-domains": async (serverId) => {
    await syncServerDomains(serverId);
  },
  "sync-app-load-balancers": (appId) => syncManagedLoadBalancersForApp(appId),
  "deploy-app": (deploymentId) => runDeployment(deploymentId),
  "cron-job": (jobId) => runCronJob(jobId),
  "database-backup": (backupId) => runDatabaseBackup(backupId),
  "database-backup-upload": (backupId) => runBackupUpload(backupId),
};
