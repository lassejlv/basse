import { connect, createServer } from "node:net";
import { chmod, mkdtemp, rm, writeFile } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join } from "node:path";

// Low-level SSH transport for the control plane. We shell out to the system
// `ssh` (and `ssh-keygen`) via Bun.spawn rather than using a JS SSH library,
// which is the most reliable approach under Bun. Requires openssh-client
// (present in dev and installed in the control-plane image).

export type SshConnection = {
  host: string;
  port: number;
  user: string;
  /** Decrypted OpenSSH private key (PEM). */
  privateKey: string;
};

export type RunResult = {
  exitCode: number;
  output: string;
};

const DEFAULT_CONNECT_TIMEOUT_SECONDS = 15;

/**
 * Materializes the private key into a 0600 temp file (plus a per-connection
 * known_hosts), runs fn with their paths, and always cleans up.
 */
async function withKeyMaterial<T>(
  conn: SshConnection,
  fn: (paths: { keyPath: string; knownHostsPath: string }) => Promise<T>,
): Promise<T> {
  const dir = await mkdtemp(join(tmpdir(), "basse-ssh-"));
  const keyPath = join(dir, "key");
  const knownHostsPath = join(dir, "known_hosts");

  try {
    await writeFile(keyPath, `${conn.privateKey.trimEnd()}\n`, { mode: 0o600 });
    await chmod(keyPath, 0o600);
    return await fn({ keyPath, knownHostsPath });
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
}

function baseArgs(
  conn: SshConnection,
  keyPath: string,
  knownHostsPath: string,
  connectTimeout: number,
): string[] {
  return [
    "-i",
    keyPath,
    "-p",
    String(conn.port),
    "-o",
    "BatchMode=yes",
    "-o",
    "StrictHostKeyChecking=accept-new",
    "-o",
    `UserKnownHostsFile=${knownHostsPath}`,
    "-o",
    "LogLevel=ERROR",
    "-o",
    `ConnectTimeout=${connectTimeout}`,
    "-o",
    "ServerAliveInterval=10",
  ];
}

function target(conn: SshConnection): string {
  return `${conn.user}@${conn.host}`;
}

/** Reads the SHA256 host-key fingerprint from a populated known_hosts file. */
async function readFingerprint(knownHostsPath: string): Promise<string | null> {
  const proc = Bun.spawn(["ssh-keygen", "-lf", knownHostsPath], {
    stdout: "pipe",
    stderr: "pipe",
  });
  const exitCode = await proc.exited;
  if (exitCode !== 0) {
    return null;
  }
  const out = (await new Response(proc.stdout).text()).trim();
  const firstLine = out.split("\n")[0] ?? "";
  const match = firstLine.match(/SHA256:[A-Za-z0-9+/=]+/);
  return match ? match[0] : null;
}

/**
 * Verifies the server is reachable over SSH with the key, capturing the host
 * key fingerprint (accept-new TOFU). Never throws — returns a result object.
 */
export async function probeReachable(
  conn: SshConnection,
  options: { timeoutMs?: number } = {},
): Promise<{ ok: boolean; fingerprint: string | null; error?: string }> {
  return withKeyMaterial(conn, async ({ keyPath, knownHostsPath }) => {
    const proc = Bun.spawn(
      [
        "ssh",
        ...baseArgs(conn, keyPath, knownHostsPath, DEFAULT_CONNECT_TIMEOUT_SECONDS),
        target(conn),
        "true",
      ],
      {
        stdout: "pipe",
        stderr: "pipe",
        signal: AbortSignal.timeout(options.timeoutMs ?? 20_000),
      },
    );

    const stderr = await new Response(proc.stderr).text();
    const exitCode = await proc.exited;
    const fingerprint = await readFingerprint(knownHostsPath);

    if (exitCode !== 0) {
      return { ok: false, fingerprint, error: stderr.trim() || `ssh exited ${exitCode}` };
    }

    return { ok: true, fingerprint };
  });
}

/**
 * Runs a script on the remote host via `bash -s` over stdin (the script is
 * never interpolated into argv). Streams combined stdout/stderr line-by-line to
 * onLine and returns the exit code + full output.
 */
export async function runScript(
  conn: SshConnection,
  script: string,
  options: { onLine?: (line: string) => void; timeoutMs?: number } = {},
): Promise<RunResult> {
  return withKeyMaterial(conn, async ({ keyPath, knownHostsPath }) => {
    const proc = Bun.spawn(
      [
        "ssh",
        ...baseArgs(conn, keyPath, knownHostsPath, DEFAULT_CONNECT_TIMEOUT_SECONDS),
        target(conn),
        "bash -s",
      ],
      {
        stdin: new TextEncoder().encode(script),
        stdout: "pipe",
        stderr: "pipe",
        signal: AbortSignal.timeout(options.timeoutMs ?? 300_000),
      },
    );

    const output = await streamLines([proc.stdout, proc.stderr], options.onLine);
    const exitCode = await proc.exited;

    return { exitCode, output };
  });
}

/**
 * Writes content to a remote file with restrictive perms. The content is piped
 * over stdin (`umask 077; cat > path`) so secrets never appear in argv/ps.
 */
export async function writeRemoteFile(
  conn: SshConnection,
  remotePath: string,
  content: string,
  options: { mode?: string; timeoutMs?: number } = {},
): Promise<void> {
  const mode = options.mode ?? "600";

  const result = await withKeyMaterial(conn, async ({ keyPath, knownHostsPath }) => {
    const command = `set -e; mkdir -p "$(dirname '${remotePath}')"; umask 077; cat > '${remotePath}'; chmod ${mode} '${remotePath}'`;
    const proc = Bun.spawn(
      [
        "ssh",
        ...baseArgs(conn, keyPath, knownHostsPath, DEFAULT_CONNECT_TIMEOUT_SECONDS),
        target(conn),
        command,
      ],
      {
        stdin: new TextEncoder().encode(content),
        stdout: "pipe",
        stderr: "pipe",
        signal: AbortSignal.timeout(options.timeoutMs ?? 30_000),
      },
    );

    const stderr = await new Response(proc.stderr).text();
    const exitCode = await proc.exited;
    return { exitCode, stderr };
  });

  if (result.exitCode !== 0) {
    throw new Error(`Failed to write ${remotePath}: ${result.stderr.trim()}`);
  }
}

export async function uploadDirectory(
  conn: SshConnection,
  localDir: string,
  remoteDir: string,
  options: { timeoutMs?: number } = {},
): Promise<void> {
  await withKeyMaterial(conn, async ({ keyPath, knownHostsPath }) => {
    const tar = Bun.spawn(["tar", "-C", localDir, "-czf", "-", "."], {
      stdout: "pipe",
      stderr: "pipe",
      signal: AbortSignal.timeout(options.timeoutMs ?? 300_000),
    });
    const archive = await new Response(tar.stdout).arrayBuffer();
    const tarStderr = await new Response(tar.stderr).text();
    const tarExit = await tar.exited;
    if (tarExit !== 0) {
      throw new Error(`Failed to archive build context: ${tarStderr.trim()}`);
    }

    const command = `set -euo pipefail; rm -rf '${remoteDir}'; mkdir -p '${remoteDir}'; tar -xzf - -C '${remoteDir}'`;
    const ssh = Bun.spawn(
      [
        "ssh",
        ...baseArgs(conn, keyPath, knownHostsPath, DEFAULT_CONNECT_TIMEOUT_SECONDS),
        target(conn),
        command,
      ],
      {
        stdin: new Uint8Array(archive),
        stdout: "pipe",
        stderr: "pipe",
        signal: AbortSignal.timeout(options.timeoutMs ?? 300_000),
      },
    );
    const stderr = await new Response(ssh.stderr).text();
    const exitCode = await ssh.exited;
    if (exitCode !== 0) {
      throw new Error(`Failed to upload build context: ${stderr.trim()}`);
    }
  });
}

/** Picks a free localhost TCP port by binding to :0 and releasing it. */
function freeLocalPort(): Promise<number> {
  return new Promise((resolve, reject) => {
    const srv = createServer();
    srv.on("error", reject);
    srv.listen(0, "127.0.0.1", () => {
      const address = srv.address();
      if (address && typeof address === "object") {
        const { port } = address;
        srv.close(() => resolve(port));
      } else {
        srv.close(() => reject(new Error("could not determine free port")));
      }
    });
  });
}

/** Resolves once a TCP connection to 127.0.0.1:port succeeds, or rejects on timeout. */
async function waitForLocalPort(port: number, timeoutMs: number): Promise<void> {
  const deadline = Date.now() + timeoutMs;

  while (Date.now() < deadline) {
    const ok = await new Promise<boolean>((resolve) => {
      const socket = connect({ host: "127.0.0.1", port });
      socket.setTimeout(1000);
      socket.once("connect", () => {
        socket.destroy();
        resolve(true);
      });
      socket.once("timeout", () => {
        socket.destroy();
        resolve(false);
      });
      socket.once("error", () => resolve(false));
    });

    if (ok) {
      return;
    }

    await Bun.sleep(150);
  }

  throw new Error(`tunnel local port ${port} did not open within ${timeoutMs}ms`);
}

/**
 * Opens an SSH local-port-forward to the agent's loopback port on the server,
 * runs fn with the resulting local base URL, and always tears the tunnel down.
 * This is how the control plane reaches the agent without any public port.
 */
export async function withTunnel<T>(
  conn: SshConnection,
  remotePort: number,
  fn: (localBaseUrl: string) => Promise<T>,
  options: { timeoutMs?: number } = {},
): Promise<T> {
  const localPort = await freeLocalPort();

  return withKeyMaterial(conn, async ({ keyPath, knownHostsPath }) => {
    const proc = Bun.spawn(
      [
        "ssh",
        ...baseArgs(conn, keyPath, knownHostsPath, DEFAULT_CONNECT_TIMEOUT_SECONDS),
        "-N",
        "-L",
        `127.0.0.1:${localPort}:127.0.0.1:${remotePort}`,
        target(conn),
      ],
      { stdout: "pipe", stderr: "pipe" },
    );

    try {
      await waitForLocalPort(localPort, options.timeoutMs ?? 15_000);
      return await fn(`http://127.0.0.1:${localPort}`);
    } finally {
      proc.kill();
      await proc.exited.catch(() => {});
    }
  });
}

/** Reads multiple byte streams concurrently, splits into lines, returns the joined output. */
async function streamLines(
  streams: ReadableStream<Uint8Array>[],
  onLine?: (line: string) => void,
): Promise<string> {
  let full = "";

  await Promise.all(
    streams.map(async (stream) => {
      const decoder = new TextDecoder();
      let buffer = "";

      for await (const chunk of stream) {
        const text = decoder.decode(chunk, { stream: true });
        buffer += text;
        full += text;
        let newlineIndex = buffer.indexOf("\n");
        while (newlineIndex !== -1) {
          const line = buffer.slice(0, newlineIndex);
          buffer = buffer.slice(newlineIndex + 1);
          if (onLine) {
            onLine(line);
          }
          newlineIndex = buffer.indexOf("\n");
        }
      }

      if (buffer.length > 0 && onLine) {
        onLine(buffer);
      }
    }),
  );

  return full;
}
