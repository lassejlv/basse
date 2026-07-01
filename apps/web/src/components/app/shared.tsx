import { useQuery } from "@tanstack/react-query";
import { useRef, useState } from "react";
import type { DatabaseKind, Deployment } from "@basse/shared";
import type { App } from "@/lib/apps";
import { listDomains } from "@/lib/domains";

export const IN_FLIGHT: Deployment["status"][] = ["queued", "building", "deploying"];

export function SpecDivider() {
  return (
    <span aria-hidden className="text-muted-foreground/40">
      ·
    </span>
  );
}

export function databaseDefaultPort(kind: DatabaseKind) {
  return kind === "redis" ? 6379 : 5432;
}

/** First active domain for this app on its single attached server, if any. */
export function useLiveUrl(app: App): string | null {
  const serverId = app.serverIds.length === 1 ? app.serverIds[0]! : null;
  const domains = useQuery({
    queryKey: ["domains", serverId],
    queryFn: () => listDomains(serverId!),
    enabled: Boolean(serverId),
  });
  if (!serverId) return null;
  const active = (domains.data ?? []).find((d) => d.appId === app.id && d.status === "active");
  return active ? `https://${active.host}` : null;
}

export function useClipboard() {
  const [copiedId, setCopiedId] = useState<string | null>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);

  function copy(id: string, text: string) {
    void navigator.clipboard.writeText(text).then(() => {
      setCopiedId(id);
      if (timer.current) clearTimeout(timer.current);
      timer.current = setTimeout(() => setCopiedId(null), 1500);
    });
  }

  return { copiedId, copy };
}

export function formatCpuCores(millicores: number): string {
  const cores = millicores / 1000;
  return `${Number.isInteger(cores) ? cores.toFixed(0) : cores.toFixed(2)} CPU`;
}

export function formatCpuInput(millicores: number): string {
  const cores = millicores / 1000;
  return Number.isInteger(cores) ? cores.toFixed(0) : cores.toFixed(2);
}

export function sliderToCpuInput(millicores: number): string {
  return millicores <= 0 ? "" : formatCpuInput(millicores);
}

export function sliderToMemoryInput(megabytes: number): string {
  return megabytes <= 0 ? "" : String(megabytes);
}

export function parseCpuLimit(value: string): number | null | undefined {
  if (!value.trim()) return null;
  const cores = Number(value);
  if (!Number.isFinite(cores) || cores <= 0) return undefined;
  return Math.round(cores * 1000);
}

export function parseMemoryLimit(value: string): number | null | undefined {
  if (!value.trim()) return null;
  const megabytes = Number(value);
  if (!Number.isFinite(megabytes) || megabytes <= 0) return undefined;
  return Math.round(megabytes * 1024 * 1024);
}
