import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { CheckCircleIcon, ExternalLinkIcon, ShieldAlertIcon } from "lucide-react";
import { useState } from "react";
import type { Alert } from "@/lib/alerts";
import { acknowledgeAlert, listAlerts, resolveAlert } from "@/lib/alerts";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { relativeTime } from "@/lib/format";
import { toast } from "@/lib/toast";

export const Route = createFileRoute("/_authed/alerts")({
  component: AlertsRoute,
});

type AlertFilter = "active" | "resolved" | "all";

const SEVERITY_VARIANT: Record<Alert["severity"], "info" | "warning" | "error"> = {
  info: "info",
  warning: "warning",
  critical: "error",
};

function AlertsRoute() {
  const queryClient = useQueryClient();
  const [filter, setFilter] = useState<AlertFilter>("active");
  const alerts = useQuery({
    queryKey: ["alerts", filter],
    queryFn: () => listAlerts(filter),
  });
  const list = alerts.data ?? [];

  const acknowledge = useMutation({
    mutationFn: acknowledgeAlert,
    onSuccess: async () => {
      toast.success("Alert acknowledged");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["alerts"] }),
        queryClient.invalidateQueries({ queryKey: ["alerts-overview"] }),
      ]);
    },
  });

  const resolve = useMutation({
    mutationFn: resolveAlert,
    onSuccess: async () => {
      toast.success("Alert resolved");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["alerts"] }),
        queryClient.invalidateQueries({ queryKey: ["alerts-overview"] }),
      ]);
    },
  });

  return (
    <section className="flex flex-1 flex-col gap-6 p-4 md:p-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="font-semibold text-2xl tracking-tight md:text-3xl">Alerts</h1>
          <p className="mt-1 text-muted-foreground text-sm">
            Background monitoring for servers, deployments, app containers, and resource pressure.
          </p>
        </div>
        <div className="flex rounded-lg border bg-muted/30 p-1">
          {(["active", "resolved", "all"] as const).map((value) => (
            <button
              className={`rounded-md px-3 py-1.5 text-sm transition ${
                filter === value ? "bg-background shadow-xs" : "text-muted-foreground"
              }`}
              key={value}
              onClick={() => setFilter(value)}
              type="button"
            >
              {value[0]!.toUpperCase() + value.slice(1)}
            </button>
          ))}
        </div>
      </div>

      {alerts.isPending ? (
        <Card className="p-6">
          <p className="text-muted-foreground text-sm">Loading alerts...</p>
        </Card>
      ) : list.length === 0 ? (
        <Card className="items-center gap-3 p-8 text-center">
          <CheckCircleIcon className="size-8 text-success-foreground" />
          <div>
            <h2 className="font-medium">No alerts</h2>
            <p className="mt-1 text-muted-foreground text-sm">
              Active incidents and recoveries from the background monitor will show here.
            </p>
          </div>
        </Card>
      ) : (
        <div className="flex flex-col gap-3">
          {list.map((item) => (
            <AlertCard
              acknowledge={() => acknowledge.mutate(item.id)}
              busy={acknowledge.isPending || resolve.isPending}
              key={item.id}
              item={item}
              resolve={() => resolve.mutate(item.id)}
            />
          ))}
        </div>
      )}
    </section>
  );
}

function AlertCard({
  item,
  acknowledge,
  resolve,
  busy,
}: {
  item: Alert;
  acknowledge: () => void;
  resolve: () => void;
  busy: boolean;
}) {
  return (
    <Card className="gap-4 p-5">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="min-w-0">
          <div className="flex flex-wrap items-center gap-2">
            <ShieldAlertIcon className="size-4 text-muted-foreground" />
            <Badge variant={SEVERITY_VARIANT[item.severity]}>{item.severity}</Badge>
            <Badge variant={item.status === "resolved" ? "success" : "outline"}>
              {item.status}
            </Badge>
            <span className="font-mono text-muted-foreground text-xs">{item.code}</span>
          </div>
          <h2 className="mt-3 font-semibold text-base">{item.title}</h2>
          <p className="mt-1 text-muted-foreground text-sm">{item.message}</p>
        </div>
        {item.status !== "resolved" ? (
          <div className="flex gap-2">
            {item.status === "open" ? (
              <Button disabled={busy} onClick={acknowledge} size="sm" variant="outline">
                Acknowledge
              </Button>
            ) : null}
            <Button disabled={busy} onClick={resolve} size="sm">
              Resolve
            </Button>
          </div>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-x-4 gap-y-2 border-t pt-3 text-muted-foreground text-xs">
        <span>First seen {relativeTime(item.firstSeenAt)}</span>
        <span>Last seen {relativeTime(item.lastSeenAt)}</span>
        {item.resolvedAt ? <span>Resolved {relativeTime(item.resolvedAt)}</span> : null}
        {item.serverId ? (
          <Link
            className="inline-flex items-center gap-1 text-foreground underline-offset-4 hover:underline"
            params={{ serverId: item.serverId }}
            to="/servers/$serverId"
          >
            {item.serverName ?? "Server"}
            <ExternalLinkIcon className="size-3" />
          </Link>
        ) : null}
        {item.appId ? (
          <Link
            className="inline-flex items-center gap-1 text-foreground underline-offset-4 hover:underline"
            params={{ appId: item.appId }}
            to="/apps/$appId"
          >
            {item.appName ?? "App"}
            <ExternalLinkIcon className="size-3" />
          </Link>
        ) : null}
        {item.deploymentId ? (
          <span className="font-mono">deployment {item.deploymentId.slice(0, 8)}</span>
        ) : null}
      </div>
    </Card>
  );
}
