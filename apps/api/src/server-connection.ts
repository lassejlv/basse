import type { server } from "@basse/db";
import { decryptSecret } from "./crypto";
import type { SshConnection } from "./ssh";

type ServerRow = typeof server.$inferSelect;

/** Decrypts a server row's private key into an SSH connection descriptor. */
export async function connectionFromServer(row: ServerRow): Promise<SshConnection> {
  const privateKey = await decryptSecret(row.sshPrivateKey);
  return {
    host: row.sshHost,
    port: row.sshPort,
    user: row.sshUser,
    privateKey,
  };
}
