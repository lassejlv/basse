import { db, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { getDigitalOceanDroplet, getDigitalOceanToken } from "../integrations/digitalocean";
import { provisionServer } from "./provision";

const POLL_INTERVAL_MS = 10_000;
const MAX_WAIT_MS = 10 * 60_000;
// Droplets report "active" before sshd accepts connections; give the boot a
// moment so the first provisioning attempt doesn't waste itself on a refusal.
const SSH_GRACE_MS = 20_000;

async function setError(serverId: string, message: string): Promise<void> {
  await db
    .update(server)
    .set({ status: "error", statusMessage: message, updatedAt: new Date() })
    .where(eq(server.id, serverId));
}

/**
 * Worker job for freshly created DigitalOcean servers: polls the droplet until
 * it is active with a public IPv4, writes the IP onto the server row, then runs
 * the normal SSH provisioning pipeline.
 */
export async function waitForDigitalOceanServer(serverId: string): Promise<void> {
  const [row] = await db.select().from(server).where(eq(server.id, serverId)).limit(1);
  if (!row) return;
  if (row.provider !== "digitalocean" || !row.providerResourceId) {
    await setError(serverId, "Server is not linked to a DigitalOcean droplet");
    return;
  }

  const token = await getDigitalOceanToken(row.organizationId);
  if (!token) {
    await setError(serverId, "DigitalOcean is no longer connected in this workspace");
    return;
  }

  const deadline = Date.now() + MAX_WAIT_MS;
  let publicIpv4: string | null = null;

  while (Date.now() < deadline) {
    let state;
    try {
      state = await getDigitalOceanDroplet(token, row.providerResourceId);
    } catch (error) {
      await setError(
        serverId,
        error instanceof Error ? error.message : "Couldn't read the droplet from DigitalOcean",
      );
      return;
    }

    if (state.status === "active" && state.publicIpv4) {
      publicIpv4 = state.publicIpv4;
      break;
    }

    await db
      .update(server)
      .set({
        statusMessage: `Waiting for the droplet to boot… (${state.status})`,
        updatedAt: new Date(),
      })
      .where(eq(server.id, serverId));

    await Bun.sleep(POLL_INTERVAL_MS);
  }

  if (!publicIpv4) {
    await setError(serverId, "Timed out waiting for the droplet to get a public IP");
    return;
  }

  await db
    .update(server)
    .set({
      sshHost: publicIpv4,
      statusMessage: "Droplet is up — provisioning the agent…",
      updatedAt: new Date(),
    })
    .where(eq(server.id, serverId));

  await Bun.sleep(SSH_GRACE_MS);
  await provisionServer(serverId);
}
