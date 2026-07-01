import type { server } from "@basse/db";
import type { AgentConnection } from "./agent-client";
import { decryptSecret } from "./crypto";

type ServerRow = typeof server.$inferSelect;

/** Decrypts a server row's private key into an SSH connection descriptor. */
export async function connectionFromServer(row: ServerRow): Promise<AgentConnection> {
  if (row.connectionMode === "outbound") {
    return { mode: "outbound", serverId: row.id };
  }

  const privateKey = await decryptSecret(row.sshPrivateKey);
  return {
    host: row.sshHost,
    port: row.sshPort,
    user: row.sshUser,
    privateKey,
  };
}
