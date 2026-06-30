import type { ServerStatus } from "@basse/shared";
import { Badge } from "@/components/ui/badge";

const STATUS_CONFIG: Record<
  ServerStatus,
  { label: string; variant: "outline" | "info" | "success" | "error" | "warning" }
> = {
  pending: { label: "Pending", variant: "outline" },
  provisioning: { label: "Provisioning", variant: "info" },
  active: { label: "Active", variant: "success" },
  error: { label: "Error", variant: "error" },
  unreachable: { label: "Unreachable", variant: "warning" },
};

export function ServerStatusBadge({ status }: { status: ServerStatus }) {
  const config = STATUS_CONFIG[status];
  return <Badge variant={config.variant}>{config.label}</Badge>;
}
