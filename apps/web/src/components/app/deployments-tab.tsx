import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { RotateCcwIcon } from "lucide-react";
import { useEffect, useState } from "react";
import type { Deployment } from "@basse/shared";
import { chartCssVars } from "@/components/charts/chart-context";
import { Grid } from "@/components/charts/grid";
import { Line, LineChart } from "@/components/charts/line-chart";
import { ChartTooltip } from "@/components/charts/tooltip";
import { XAxis } from "@/components/charts/x-axis";
import { DeployStatusBadge, StatusDot } from "@/components/deploy-status";
import { LogExplorer } from "@/components/log-explorer";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetDescription,
  SheetHeader,
  SheetPanel,
  SheetPopup,
  SheetTitle,
} from "@/components/ui/sheet";
import type { App } from "@/lib/apps";
import { getAppLogs, getAppMetrics } from "@/lib/apps";
import { getDeployment, rollbackDeployment } from "@/lib/deployments";
import { formatBytes, relativeTime } from "@/lib/format";
import { listServers } from "@/lib/servers";
import { toast } from "@/lib/toast";
import { formatCpuCores } from "./shared";

export function DeploymentsPanel({
  app,
  deployments,
  isPending,
}: {
  app: App;
  deployments: Deployment[];
  isPending: boolean;
}) {
  const queryClient = useQueryClient();
  const latest = deployments[0];
  const [selectedDeploymentId, setSelectedDeploymentId] = useState<string | null>(null);
  const [rollbackError, setRollbackError] = useState<string | null>(null);
  const selectedDeployment =
    deployments.find((deployment) => deployment.id === selectedDeploymentId) ?? null;

  const rollback = useMutation({
    mutationFn: (deploymentId: string) => rollbackDeployment(deploymentId),
    onSuccess: async () => {
      setRollbackError(null);
      toast.success("Rollback started");
      await queryClient.invalidateQueries({ queryKey: ["deployments", app.id] });
      await queryClient.invalidateQueries({ queryKey: ["app", app.id] });
    },
    onError: (error: Error) => setRollbackError(error.message),
  });

  return (
    <div className="flex flex-col gap-5">
      <div>
        <div className="mb-2 flex items-center justify-between gap-3">
          <h2 className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
            History
          </h2>
          {latest ? (
            <span className="text-muted-foreground text-xs">
              Click a deployment for build logs and runtime data.
            </span>
          ) : null}
        </div>
        {isPending ? (
          <div className="h-24 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
        ) : deployments.length === 0 ? (
          <p className="rounded-lg border border-dashed px-3 py-8 text-center text-muted-foreground text-sm">
            No deployments yet. Deploy starts the first release.
          </p>
        ) : (
          <Card className="divide-y overflow-hidden p-0">
            {deployments.map((deployment) => {
              const canRollback =
                deployment.id !== latest?.id &&
                Boolean(deployment.imageRef) &&
                ["healthy", "superseded"].includes(deployment.status);
              const imageTail =
                deployment.imageRef?.split("/").pop() ?? deployment.buildId ?? "no image yet";
              return (
                <div key={deployment.id} className="group flex items-center gap-2.5 px-3 py-2">
                  <StatusDot className="size-2" status={deployment.status} />
                  <button
                    className="min-w-0 flex-1 rounded-md text-left outline-none focus-visible:ring-2 focus-visible:ring-ring"
                    onClick={() => setSelectedDeploymentId(deployment.id)}
                    type="button"
                  >
                    <span className="flex items-baseline gap-2">
                      <span className="font-medium font-mono text-xs">
                        {deployment.commitSha?.slice(0, 7) ?? deployment.id.slice(0, 7)}
                      </span>
                      <span
                        className="text-muted-foreground text-xs"
                        title={new Date(deployment.createdAt).toLocaleString()}
                      >
                        {relativeTime(deployment.createdAt)}
                      </span>
                    </span>
                    <span
                      className="block truncate font-mono text-[11px] text-muted-foreground/70"
                      title={deployment.imageRef ?? undefined}
                    >
                      {imageTail}
                    </span>
                  </button>
                  <DeployStatusBadge size="sm" status={deployment.status} />
                  {canRollback ? (
                    <Button
                      aria-label="Roll back to this deployment"
                      className="opacity-0 transition group-focus-within:opacity-100 group-hover:opacity-100"
                      disabled={rollback.isPending}
                      loading={rollback.isPending && rollback.variables === deployment.id}
                      onClick={() => rollback.mutate(deployment.id)}
                      size="icon-xs"
                      title="Roll back to this deployment"
                      variant="ghost"
                    >
                      <RotateCcwIcon />
                    </Button>
                  ) : null}
                </div>
              );
            })}
          </Card>
        )}
        {rollbackError ? (
          <p className="mt-2 text-destructive-foreground text-sm">{rollbackError}</p>
        ) : null}
      </div>

      <DeploymentDetailSheet
        app={app}
        deployment={selectedDeployment}
        onOpenChange={(open) => {
          if (!open) setSelectedDeploymentId(null);
        }}
        open={Boolean(selectedDeployment)}
      />
    </div>
  );
}

function DeploymentDetailSheet({
  app,
  deployment,
  open,
  onOpenChange,
}: {
  app: App;
  deployment: Deployment | null;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  return (
    <Sheet onOpenChange={onOpenChange} open={open}>
      <SheetPopup className="max-w-4xl" side="right">
        <SheetHeader>
          <SheetTitle>{deployment ? "Deployment details" : "Deployment"}</SheetTitle>
          <SheetDescription>
            {deployment
              ? `${new Date(deployment.createdAt).toLocaleString()}${
                  deployment.commitSha ? ` · ${deployment.commitSha.slice(0, 7)}` : ""
                }`
              : "Build logs and runtime data for the selected deployment."}
          </SheetDescription>
        </SheetHeader>
        <SheetPanel className="space-y-6">
          {deployment ? (
            <>
              <DeploymentSummary deployment={deployment} />
              <DeploymentBuildLog deployment={deployment} />
              <DeploymentRuntimeSection app={app} deployment={deployment} />
            </>
          ) : null}
        </SheetPanel>
      </SheetPopup>
    </Sheet>
  );
}

function DeploymentSummary({ deployment }: { deployment: Deployment }) {
  return (
    <div className="grid gap-3 sm:grid-cols-3">
      <div className="rounded-lg border p-3">
        <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Status
        </p>
        <div className="mt-2">
          <DeployStatusBadge status={deployment.status} />
        </div>
      </div>
      <div className="rounded-lg border p-3">
        <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Image
        </p>
        <p className="mt-2 truncate font-mono text-xs" title={deployment.imageRef ?? undefined}>
          {deployment.imageRef ?? "No image recorded"}
        </p>
      </div>
      <div className="rounded-lg border p-3">
        <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Updated
        </p>
        <p className="mt-2 font-mono text-xs">{new Date(deployment.updatedAt).toLocaleString()}</p>
      </div>
    </div>
  );
}

function DeploymentBuildLog({ deployment }: { deployment: Deployment }) {
  const inFlight = ["queued", "building", "deploying"].includes(deployment.status);
  // The list endpoint only carries logs for the newest and in-flight rows;
  // older deployments load their (immutable) logs on demand.
  const detail = useQuery({
    queryKey: ["deployment", deployment.id],
    queryFn: () => getDeployment(deployment.id),
    enabled: !inFlight && deployment.logs === null,
    staleTime: Number.POSITIVE_INFINITY,
  });
  const logs = deployment.logs ?? detail.data?.logs ?? "";

  return (
    <section>
      <div className="mb-2 flex items-center justify-between gap-3">
        <h3 className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Build logs
        </h3>
        <span className="font-mono text-muted-foreground/60 text-xs">
          {deployment.buildId ?? deployment.id.slice(0, 8)}
        </span>
      </div>
      <LogExplorer
        downloadName={`build-${deployment.id.slice(0, 8)}.log`}
        emptyText={
          inFlight
            ? "Waiting for logs…"
            : detail.isFetching
              ? "Loading logs…"
              : "No build logs recorded."
        }
        live={inFlight}
        maxHeight="22rem"
        text={logs}
      />
    </section>
  );
}

function DeploymentRuntimeSection({ app, deployment }: { app: App; deployment: Deployment }) {
  const servers = useQuery({ queryKey: ["servers", "deployment-runtime"], queryFn: listServers });
  const targetServers = (servers.data ?? []).filter((server) => app.serverIds.includes(server.id));
  const [serverId, setServerId] = useState(app.serverIds[0] ?? "");
  const [samples, setSamples] = useState<
    {
      date: Date;
      cpuPercent: number;
      memoryPercent: number;
      memoryBytes: number;
      memoryLimitBytes: number;
    }[]
  >([]);
  const runtimeAvailable = deployment.status === "healthy";

  useEffect(() => {
    if (!serverId || !app.serverIds.includes(serverId)) {
      setServerId(app.serverIds[0] ?? "");
      setSamples([]);
    }
  }, [app.serverIds, serverId]);

  useEffect(() => {
    setSamples([]);
  }, [deployment.id, serverId]);

  const metrics = useQuery({
    queryKey: ["app-metrics", app.id, serverId],
    queryFn: () => getAppMetrics(app.id, serverId),
    enabled: runtimeAvailable && Boolean(serverId),
    refetchInterval: 5000,
  });

  const logs = useQuery({
    queryKey: ["app-logs", app.id, serverId],
    queryFn: () => getAppLogs(app.id, serverId),
    enabled: runtimeAvailable && Boolean(serverId),
    refetchInterval: 5000,
  });

  useEffect(() => {
    if (!metrics.data) return;
    setSamples((current) => [
      ...current.slice(-29),
      {
        date: new Date(metrics.data.timestamp),
        cpuPercent: Number(metrics.data.cpuPercent.toFixed(2)),
        memoryPercent: Number(metrics.data.memoryPercent.toFixed(2)),
        memoryBytes: metrics.data.memoryBytes,
        memoryLimitBytes: metrics.data.memoryLimitBytes,
      },
    ]);
  }, [metrics.data]);

  if (app.serverIds.length === 0) {
    return (
      <section>
        <h3 className="mb-2 font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Runtime
        </h3>
        <p className="rounded-lg border border-dashed px-3 py-6 text-center text-muted-foreground text-sm">
          Attach a server before runtime metrics and logs are available.
        </p>
      </section>
    );
  }

  if (!runtimeAvailable) {
    return (
      <section>
        <h3 className="mb-2 font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Runtime
        </h3>
        <p className="rounded-lg border border-dashed px-3 py-6 text-center text-muted-foreground text-sm">
          Runtime metrics are only available for the active healthy deployment. This deployment is{" "}
          <span className="font-medium text-foreground">{deployment.status}</span>.
        </p>
      </section>
    );
  }

  const cpuLimitMillicores = app.resourceLimits.cpuMillicores;
  const effectiveMemoryLimit = metrics.data?.memoryLimitBytes || app.resourceLimits.memoryBytes;
  const cpuProgress =
    metrics.data && cpuLimitMillicores
      ? Math.min(100, metrics.data.cpuPercent / (cpuLimitMillicores / 1000))
      : null;
  const memoryProgress =
    metrics.data && effectiveMemoryLimit
      ? Math.min(100, (metrics.data.memoryBytes / effectiveMemoryLimit) * 100)
      : null;

  return (
    <section>
      <div className="flex flex-wrap items-center justify-between gap-3">
        <h3 className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Runtime
        </h3>
        {targetServers.length > 1 ? (
          <Select value={serverId} onValueChange={(value) => setServerId(value ?? "")}>
            <SelectTrigger className="w-48">
              <SelectValue placeholder="Select server">
                {(value: string) =>
                  targetServers.find((server) => server.id === value)?.name ?? "Select server"
                }
              </SelectValue>
            </SelectTrigger>
            <SelectPopup>
              {targetServers.map((server) => (
                <SelectItem key={server.id} value={server.id}>
                  {server.name}
                </SelectItem>
              ))}
            </SelectPopup>
          </Select>
        ) : null}
      </div>

      <div className="mt-3 grid gap-3 sm:grid-cols-2">
        <UsageMetricTile
          label="CPU"
          value={metrics.data ? `${metrics.data.cpuPercent.toFixed(1)}%` : "—"}
          detail={
            cpuLimitMillicores ? `Limit ${formatCpuCores(cpuLimitMillicores)}` : "No app CPU limit"
          }
          progress={cpuProgress}
          tone="primary"
        />
        <UsageMetricTile
          label="Memory"
          value={
            metrics.data
              ? `${formatBytes(metrics.data.memoryBytes)} / ${formatBytes(effectiveMemoryLimit)}`
              : "—"
          }
          detail={
            app.resourceLimits.memoryBytes
              ? `Limit ${formatBytes(app.resourceLimits.memoryBytes)}`
              : "No app memory limit"
          }
          progress={memoryProgress}
          tone="secondary"
        />
      </div>

      <div className="mt-3 overflow-hidden rounded-lg border">
        <LineChart
          animationDuration={700}
          aspectRatio={undefined}
          className="h-56"
          data={samples}
          loadingLabel={metrics.isError ? "Metrics unavailable" : "Collecting metrics"}
          margin={{ bottom: 28, left: 12, right: 12, top: 16 }}
          status={samples.length > 1 ? "ready" : "loading"}
          xDataKey="date"
        >
          <Grid horizontal />
          <Line dataKey="cpuPercent" stroke={chartCssVars.linePrimary} strokeWidth={2} />
          <Line dataKey="memoryPercent" stroke={chartCssVars.lineSecondary} strokeWidth={2} />
          <XAxis />
          <ChartTooltip
            rows={(point) => [
              {
                color: chartCssVars.linePrimary,
                label: "CPU",
                value: `${Number(point.cpuPercent).toFixed(1)}%`,
              },
              {
                color: chartCssVars.lineSecondary,
                label: "Memory",
                value: `${formatBytes(Number(point.memoryBytes))} / ${formatBytes(
                  Number(point.memoryLimitBytes),
                )}`,
              },
            ]}
          />
        </LineChart>
        <div className="flex gap-4 border-t px-3 py-2 text-muted-foreground text-xs">
          <span className="inline-flex items-center gap-1.5">
            <span
              className="size-2 rounded-full"
              style={{ background: chartCssVars.linePrimary }}
            />
            CPU
          </span>
          <span className="inline-flex items-center gap-1.5">
            <span
              className="size-2 rounded-full"
              style={{ background: chartCssVars.lineSecondary }}
            />
            Memory
          </span>
        </div>
      </div>

      <div className="mt-5">
        <h3 className="mb-2 font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          Runtime logs
        </h3>
        <LogExplorer
          downloadName={`${app.slug}-runtime.log`}
          emptyText={logs.isError ? "Logs unavailable." : "No logs yet."}
          isRefreshing={logs.isFetching}
          live={!logs.isError}
          maxHeight="20rem"
          onRefresh={() => void logs.refetch()}
          text={logs.isError ? "" : (logs.data?.logs ?? "")}
        />
      </div>
    </section>
  );
}

function UsageMetricTile({
  label,
  value,
  detail,
  progress,
  tone,
}: {
  label: string;
  value: string;
  detail: string;
  progress: number | null;
  tone: "primary" | "secondary";
}) {
  const barColor = tone === "primary" ? chartCssVars.linePrimary : chartCssVars.lineSecondary;
  return (
    <div className="rounded-lg border p-3">
      <div className="flex items-baseline justify-between gap-2">
        <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
          {label}
        </p>
        <p className="truncate text-muted-foreground text-xs">{detail}</p>
      </div>
      <p className="mt-1.5 truncate font-mono text-sm">{value}</p>
      <div className="mt-2 h-1 overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{ width: `${progress ?? 0}%`, background: barColor }}
        />
      </div>
    </div>
  );
}
