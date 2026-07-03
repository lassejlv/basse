import { db, server } from "@basse/db";
import { eq } from "drizzle-orm";
import { deleteCloudflareARecord, upsertCloudflareARecord } from "../integrations/cloudflare";

const HOST_PATTERN = /^(?:[a-z0-9](?:[a-z0-9-]{0,61}[a-z0-9])?\.)+[a-z]{2,63}$/;

export function cloudPreviewRootDomain(): string | null {
  const configured = Bun.env.CLOUD_PREVIEW_URL?.trim().toLowerCase();
  if (!configured) return null;

  const withoutProtocol = configured.replace(/^https?:\/\//, "");
  const host = withoutProtocol.split("/")[0]?.replace(/\.$/, "") ?? "";
  return HOST_PATTERN.test(host) ? host : null;
}

export function cloudPreviewEnabled(): boolean {
  return Boolean(cloudPreviewRootDomain() && cloudPreviewToken());
}

export function cloudPreviewReservedHostMessage(): string {
  const root = cloudPreviewRootDomain() ?? "the cloud preview domain";
  return `Use the app preview domain generator for ${root} subdomains`;
}

export function generatedCloudPreviewHost(appSlug: string, appId: string): string | null {
  const root = cloudPreviewRootDomain();
  if (!root) return null;
  const slug = appSlug
    .trim()
    .toLowerCase()
    .replace(/[^a-z0-9-]+/g, "-")
    .replace(/^-+|-+$/g, "");
  return `${slug || "app"}-${appId.slice(0, 8)}.${root}`;
}

export function isCloudPreviewHost(host: string): boolean {
  const root = cloudPreviewRootDomain();
  if (!root) return false;
  const normalized = host.trim().toLowerCase().replace(/\.$/, "");
  return normalized === root || normalized.endsWith(`.${root}`);
}

export async function upsertCloudPreviewDns(host: string, serverId: string): Promise<void> {
  if (!isCloudPreviewHost(host)) return;

  const token = cloudPreviewToken();
  if (!token) {
    throw new Error("Cloud preview Cloudflare token is not configured");
  }

  const [row] = await db
    .select({ sshHost: server.sshHost })
    .from(server)
    .where(eq(server.id, serverId))
    .limit(1);
  if (!row) {
    throw new Error("Cloud preview target server was not found");
  }

  const address = normalizeIpv4(row.sshHost);
  if (!address) {
    throw new Error("Cloud preview domains require the target server to use an IPv4 SSH host");
  }

  await upsertCloudflareARecord(token, host.trim().toLowerCase().replace(/\.$/, ""), address);
}

export async function deleteCloudPreviewDns(host: string): Promise<void> {
  if (!isCloudPreviewHost(host)) return;

  const token = cloudPreviewToken();
  if (!token) {
    throw new Error("Cloud preview Cloudflare token is not configured");
  }

  await deleteCloudflareARecord(token, host.trim().toLowerCase().replace(/\.$/, ""));
}

function cloudPreviewToken(): string | null {
  return Bun.env.CLOUD_PREVIEW_CLOUDFLARE_API_TOKEN?.trim() || null;
}

function normalizeIpv4(value: string): string | null {
  const parts = value.trim().split(".");
  if (parts.length !== 4) return null;
  const octets = parts.map((part) => {
    if (!/^\d{1,3}$/.test(part)) return null;
    const n = Number(part);
    return Number.isInteger(n) && n >= 0 && n <= 255 ? n : null;
  });
  if (octets.some((part) => part === null)) return null;
  return octets.join(".");
}
