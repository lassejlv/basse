import type React from "react";
import type { DeploymentStatus } from "@basse/shared";
import { Badge } from "@/components/ui/badge";
import { cn } from "@/lib/utils";

// The deploy "heartbeat" — the dashboard's signature element. A status maps to a
// tone + label; in-flight states pulse so a running deploy reads as alive at a
// glance, while settled states stay still. Motion is suppressed under
// prefers-reduced-motion.
type Tone = "live" | "progress" | "failed" | "idle";

type DeployState = { label: string; tone: Tone; pulse: boolean };

export function deployState(status: DeploymentStatus | null | undefined): DeployState {
  switch (status) {
    case "healthy":
      return { label: "Live", tone: "live", pulse: false };
    case "queued":
      return { label: "Queued", tone: "progress", pulse: true };
    case "building":
      return { label: "Building", tone: "progress", pulse: true };
    case "deploying":
      return { label: "Deploying", tone: "progress", pulse: true };
    case "failed":
      return { label: "Failed", tone: "failed", pulse: false };
    case "crashed":
      return { label: "Crashed", tone: "failed", pulse: false };
    case "stopped":
      return { label: "Stopped", tone: "idle", pulse: false };
    case "cancelled":
      return { label: "Cancelled", tone: "idle", pulse: false };
    case "superseded":
      return { label: "Superseded", tone: "idle", pulse: false };
    default:
      return { label: "Not deployed", tone: "idle", pulse: false };
  }
}

const dotColor: Record<Tone, string> = {
  live: "bg-success",
  progress: "bg-info",
  failed: "bg-destructive",
  idle: "bg-muted-foreground/50",
};

const badgeVariant: Record<Tone, "success" | "info" | "error" | "secondary"> = {
  live: "success",
  progress: "info",
  failed: "error",
  idle: "secondary",
};

export function StatusDot({
  status,
  className,
}: {
  status: DeploymentStatus | null | undefined;
  className?: string;
}): React.ReactElement {
  const { tone, pulse } = deployState(status);
  return (
    <span className={cn("relative inline-flex size-2 shrink-0", className)}>
      {pulse ? (
        <span
          className={cn(
            "absolute inline-flex size-full animate-ping rounded-full opacity-60 motion-reduce:hidden",
            dotColor[tone],
          )}
        />
      ) : null}
      <span className={cn("relative inline-flex size-full rounded-full", dotColor[tone])} />
    </span>
  );
}

export function DeployStatusBadge({
  status,
  size,
}: {
  status: DeploymentStatus | null | undefined;
  size?: "default" | "lg" | "sm";
}): React.ReactElement {
  const { label, tone } = deployState(status);
  return (
    <Badge size={size} variant={badgeVariant[tone]}>
      <StatusDot className="size-1.5" status={status} />
      {label}
    </Badge>
  );
}
