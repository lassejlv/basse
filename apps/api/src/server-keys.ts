import { mkdtemp, readFile, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

export type GeneratedKeyPair = {
  publicKey: string;
  privateKey: string;
};

/**
 * Generates a per-server ed25519 keypair via `ssh-keygen`. The public key is in
 * authorized_keys format (`ssh-ed25519 AAAA… comment`); the private key is the
 * OpenSSH private key. We shell out rather than hand-encoding the OpenSSH wire
 * format. Both files are written to a temp dir that is always removed.
 *
 * Requires the `openssh-client` package (ssh-keygen) — present in dev and added
 * to the control-plane Docker image.
 */
export async function generateServerKeyPair(serverId: string): Promise<GeneratedKeyPair> {
  const dir = await mkdtemp(join(tmpdir(), "basse-keygen-"));
  const keyPath = join(dir, "id_ed25519");

  try {
    const proc = Bun.spawn(
      [
        "ssh-keygen",
        "-t",
        "ed25519",
        "-N",
        "",
        "-C",
        `basse-${serverId}`,
        "-f",
        keyPath,
      ],
      { stdout: "pipe", stderr: "pipe" },
    );

    const exitCode = await proc.exited;

    if (exitCode !== 0) {
      const stderr = await new Response(proc.stderr).text();
      throw new Error(`ssh-keygen failed (${exitCode}): ${stderr.trim()}`);
    }

    const [privateKey, publicKey] = await Promise.all([
      readFile(keyPath, "utf8"),
      readFile(`${keyPath}.pub`, "utf8"),
    ]);

    return { publicKey: publicKey.trim(), privateKey: privateKey.trim() };
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}
