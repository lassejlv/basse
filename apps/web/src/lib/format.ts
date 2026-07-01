// Compact relative time ("just now", "5m ago", "3d ago") for created/updated
// timestamps shown across the dashboard.
export function relativeTime(iso: string): string {
  const then = new Date(iso).getTime();
  if (Number.isNaN(then)) return "";

  const seconds = Math.round((Date.now() - then) / 1000);
  if (seconds < 45) return "just now";

  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;

  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;

  const days = Math.round(hours / 24);
  if (days < 30) return `${days}d ago`;

  const months = Math.round(days / 30);
  if (months < 12) return `${months}mo ago`;

  return `${Math.round(months / 12)}y ago`;
}

export function formatBytes(value: number | null | undefined): string {
  if (!value || !Number.isFinite(value) || value <= 0) return "0 B";
  const units = ["B", "KB", "MB", "GB", "TB"];
  let size = value;
  let unitIndex = 0;

  while (size >= 1024 && unitIndex < units.length - 1) {
    size /= 1024;
    unitIndex += 1;
  }

  const precision = size >= 10 || unitIndex === 0 ? 0 : 1;
  return `${size.toFixed(precision)} ${units[unitIndex]}`;
}

/** Redact a host for list surfaces — the full address belongs on the server
 * detail page only. IPs keep the first two octets, hostnames the first label. */
export function maskHost(host: string): string {
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(host)) {
    const octets = host.split(".");
    return `${octets[0]}.${octets[1]}.•••.•••`;
  }
  const labels = host.split(".");
  if (labels.length <= 1) {
    return host.length > 4 ? `${host.slice(0, 4)}•••` : host;
  }
  return `${labels[0]}.•••`;
}
