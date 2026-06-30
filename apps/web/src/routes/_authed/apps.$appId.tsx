import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  CheckIcon,
  CopyIcon,
  ExternalLinkIcon,
  EyeIcon,
  EyeOffIcon,
  PencilIcon,
  PlusIcon,
  RotateCcwIcon,
  RocketIcon,
  TrashIcon,
} from "lucide-react";
import { FormEvent, useEffect, useRef, useState } from "react";
import type {
  AppBuildRunner,
  AppSourceType,
  AppVolume,
  DatabaseKind,
  Deployment,
  ManagedLoadBalancer,
} from "@basse/shared";
import { chartCssVars } from "@/components/charts/chart-context";
import { Grid } from "@/components/charts/grid";
import { Line, LineChart } from "@/components/charts/line-chart";
import { ChartTooltip } from "@/components/charts/tooltip";
import { XAxis } from "@/components/charts/x-axis";
import { DatabaseIcon, databaseEngineLabel } from "@/components/database-icon";
import { DeployStatusBadge, StatusDot } from "@/components/deploy-status";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Slider } from "@/components/ui/slider";
import {
  Sheet,
  SheetDescription,
  SheetHeader,
  SheetPanel,
  SheetPopup,
  SheetTitle,
} from "@/components/ui/sheet";
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import { StagedChangesBar } from "@/components/staged-changes-bar";
import type { App } from "@/lib/apps";
import {
  getApp,
  getDatabaseConnectionInfo,
  getAppLogs,
  getAppMetrics,
  deleteApp,
} from "@/lib/apps";
import type { StagedChange } from "@/lib/changes";
import { getChanges, getEnvDraft, stageAppChanges, stageEnvVars } from "@/lib/changes";
import { listDeployments, rollbackDeployment, triggerDeploy } from "@/lib/deployments";
import { createDomain, deleteDomain, listDomains } from "@/lib/domains";
import { parseDotenv, serializeDotenv } from "@/lib/dotenv";
import { listEnvVars, revealEnvVars } from "@/lib/env-vars";
import { formatBytes } from "@/lib/format";
import {
  createManagedLoadBalancer,
  deleteManagedLoadBalancer,
  listLoadBalancerIntegrations,
  listManagedLoadBalancers,
  syncManagedLoadBalancer,
} from "@/lib/load-balancers";
import { getAgentInfo, listServers } from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";

export const Route = createFileRoute("/_authed/apps/$appId")({
  component: AppDetailRoute,
});

const IN_FLIGHT: Deployment["status"][] = ["queued", "building", "deploying"];

function AppDetailRoute() {
  const { appId } = Route.useParams();
  const app = useQuery({ queryKey: ["app", appId], queryFn: () => getApp(appId) });

  const deployments = useQuery({
    queryKey: ["deployments", appId],
    queryFn: () => listDeployments(appId),
    // Poll while the newest deployment is still in flight.
    refetchInterval: (query) => {
      const latest = query.state.data?.[0];
      return latest && IN_FLIGHT.includes(latest.status) ? 2000 : false;
    },
  });

  // Staged ("uncommitted") changes for this app, persisted server-side so they
  // survive reloads. `draft` is the live app with the staged config overlaid —
  // settings forms seed from it, while the header/deployments show live state.
  const changes = useQuery({ queryKey: ["changes", appId], queryFn: () => getChanges(appId) });

  if (app.isPending) {
    return <p className="p-4 text-muted-foreground text-sm md:p-6">Loading…</p>;
  }
  if (app.isError || !app.data) {
    return <p className="p-4 text-destructive-foreground text-sm md:p-6">App not found.</p>;
  }

  const data = app.data;
  const draft = changes.data?.draft ?? data;
  const stagedChanges = changes.data?.changes ?? [];
  const list = deployments.data ?? [];
  const status = list[0]?.status ?? data.latestDeploymentStatus ?? null;
  const canDeploy =
    data.appKind === "database"
      ? data.serverIds.length === 1
      : data.serverIds.length > 0 &&
        (data.sourceType === "image" ||
          data.buildRunner !== "server" ||
          data.serverIds.length === 1);

  return (
    <section className="flex flex-1 flex-col p-4 md:p-6">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-6">
        <Breadcrumb app={data} />
        <AppHeader
          app={data}
          appId={appId}
          canDeploy={canDeploy}
          hasStagedChanges={stagedChanges.length > 0}
          status={status}
        />

        <Tabs defaultValue="deployments">
          <TabsList variant="underline" className="w-full justify-start overflow-x-auto">
            <TabsTab value="deployments">Deployments</TabsTab>
            {data.appKind === "database" ? <TabsTab value="connection">Connection</TabsTab> : null}
            {data.appKind === "service" ? <TabsTab value="variables">Variables</TabsTab> : null}
            {data.appKind === "service" ? <TabsTab value="domains">Domains</TabsTab> : null}
            <TabsTab value="settings">Settings</TabsTab>
          </TabsList>

          <TabsPanel className="pt-5" value="deployments">
            <DeploymentsPanel app={data} deployments={list} isPending={deployments.isPending} />
          </TabsPanel>
          {data.appKind === "database" ? (
            <TabsPanel className="pt-5" value="connection">
              <DatabaseConnectionCard app={data} />
            </TabsPanel>
          ) : null}
          {data.appKind === "service" ? (
            <TabsPanel className="pt-5" value="variables">
              <EnvVarsCard appId={appId} stagedChanges={stagedChanges} />
            </TabsPanel>
          ) : null}
          {data.appKind === "service" ? (
            <TabsPanel className="pt-5" value="domains">
              {data.serverIds.length === 1 ? (
                <AppDomainsSection app={data} serverId={data.serverIds[0]!} />
              ) : data.serverIds.length > 1 ? (
                <ManagedLoadBalancerSection app={data} />
              ) : (
                <Card className="p-6">
                  <p className="text-muted-foreground text-sm">
                    Attach a server to this app to route a domain to it.
                  </p>
                </Card>
              )}
            </TabsPanel>
          ) : null}
          <TabsPanel className="flex flex-col gap-6 pt-5" value="settings">
            {data.appKind === "database" ? (
              <DatabaseSettingsCard app={draft} />
            ) : (
              <BuildSettingsCard app={draft} />
            )}
            <ServerCard app={draft} />
            <ResourceLimitsCard app={draft} />
            {data.appKind === "service" ? <VolumesCard app={draft} /> : null}
            <DeleteAppCard app={data} />
          </TabsPanel>
        </Tabs>

        <StagedChangesBar appId={appId} changes={stagedChanges} />
      </div>
    </section>
  );
}

function DeleteAppCard({ app }: { app: App }) {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const remove = useMutation({
    mutationFn: () => deleteApp(app.id),
    onSuccess: async () => {
      setError(null);
      toast.success("App deleted");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["app", app.id] }),
        queryClient.invalidateQueries({ queryKey: ["apps", app.environmentId] }),
      ]);
      if (app.projectId) {
        await navigate({ to: "/projects/$projectId", params: { projectId: app.projectId } });
      } else {
        await navigate({ to: "/projects" });
      }
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function confirmDelete() {
    if (!window.confirm(`Delete ${app.name}? This removes the app and its running containers.`)) {
      return;
    }
    remove.mutate();
  }

  return (
    <Card className="border-destructive/30 p-6">
      <h2 className="font-semibold text-lg">Delete app</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Remove this app, its running containers, deployment history, variables, and server
        assignments.
      </p>
      {error ? <p className="mt-3 text-destructive-foreground text-sm">{error}</p> : null}
      <Button
        className="mt-4"
        loading={remove.isPending}
        onClick={confirmDelete}
        variant="destructive"
      >
        <TrashIcon />
        Delete app
      </Button>
    </Card>
  );
}

function Breadcrumb({ app }: { app: App }) {
  return (
    <nav className="flex items-center gap-1.5 text-muted-foreground text-sm">
      <Link
        className="inline-flex items-center gap-1.5 transition hover:text-foreground"
        to="/projects"
      >
        <ArrowLeftIcon className="size-4" />
        Projects
      </Link>
      {app.projectId ? (
        <>
          <span aria-hidden className="text-muted-foreground/40">
            /
          </span>
          <Link
            className="transition hover:text-foreground"
            params={{ projectId: app.projectId }}
            to="/projects/$projectId"
          >
            {app.projectName}
          </Link>
        </>
      ) : null}
      {app.environmentName ? (
        <>
          <span aria-hidden className="text-muted-foreground/40">
            /
          </span>
          <span className="font-mono text-xs">{app.environmentName}</span>
        </>
      ) : null}
    </nav>
  );
}

function AppHeader({
  app,
  appId,
  canDeploy,
  status,
  hasStagedChanges,
}: {
  app: App;
  appId: string;
  canDeploy: boolean;
  status: Deployment["status"] | null;
  hasStagedChanges: boolean;
}) {
  const repoHost = app.repositoryUrl.replace(/^https?:\/\//, "");
  const liveUrl = useLiveUrl(app);
  const database = app.database;

  return (
    <Card className="gap-5 p-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <StatusDot className="size-3" status={status} />
          <h1 className="truncate font-semibold text-2xl tracking-tight">{app.name}</h1>
          <DeployStatusBadge status={status} />
        </div>
        <DeployButton appId={appId} canDeploy={canDeploy} hasStagedChanges={hasStagedChanges} />
      </div>

      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-muted-foreground text-xs">
        {database ? (
          <>
            <span className="inline-flex items-center gap-1.5 text-foreground/80">
              <DatabaseIcon className="size-4" kind={database.kind} />
              <span>
                {databaseEngineLabel(database.kind)} {database.version}
              </span>
            </span>
            <SpecDivider />
            <span>
              {database.internalHost}:{database.internalPort}
            </span>
          </>
        ) : app.sourceType === "image" ? (
          <span className="text-foreground/80">{app.imageRef}</span>
        ) : (
          <a
            className="inline-flex items-center gap-1 text-foreground/80 underline-offset-2 hover:underline"
            href={app.repositoryUrl}
            rel="noreferrer"
            target="_blank"
          >
            {repoHost}
            <ExternalLinkIcon className="size-3 opacity-70" />
          </a>
        )}
        {database ? null : <SpecDivider />}
        {!database && app.sourceType === "repository" ? (
          <>
            <span>{app.branch}</span>
            <SpecDivider />
          </>
        ) : null}
        {!database ? <span>:{app.port}</span> : null}
        {!database && app.sourceType === "repository" ? (
          <>
            <SpecDivider />
            <span>{app.buildRunner}</span>
          </>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-3 border-t pt-4 text-sm">
        {database ? (
          <span className="text-muted-foreground">
            {database.publicEnabled && database.publicPort
              ? `Public TCP enabled on port ${database.publicPort}`
              : "Private internal database"}
          </span>
        ) : liveUrl ? (
          <a
            className="inline-flex items-center gap-1.5 font-medium text-foreground underline-offset-4 hover:underline"
            href={liveUrl}
            rel="noreferrer"
            target="_blank"
          >
            {liveUrl.replace(/^https?:\/\//, "")}
            <ExternalLinkIcon className="size-3.5 opacity-70" />
          </a>
        ) : (
          <span className="text-muted-foreground">No domain attached</span>
        )}
      </div>

      {app.serverIds.length === 0 ? (
        <p className="text-warning-foreground text-sm">
          No servers attached — open Settings and select at least one before deploying.
        </p>
      ) : null}
    </Card>
  );
}

function SpecDivider() {
  return (
    <span aria-hidden className="text-muted-foreground/40">
      ·
    </span>
  );
}

function databaseDefaultPort(kind: DatabaseKind) {
  return kind === "redis" ? 6379 : 5432;
}

/** First active domain for this app on its single attached server, if any. */
function useLiveUrl(app: App): string | null {
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

function DeployButton({
  appId,
  canDeploy,
  hasStagedChanges,
}: {
  appId: string;
  canDeploy: boolean;
  hasStagedChanges: boolean;
}) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const deploy = useMutation({
    mutationFn: () => triggerDeploy(appId),
    onSuccess: async () => {
      setError(null);
      toast.success("Deployment queued");
      await queryClient.invalidateQueries({ queryKey: ["deployments", appId] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <div className="flex flex-col items-end gap-1">
      {/* While changes are staged, deploying goes through the staged-changes bar
          (apply + deploy); this button would otherwise ship the live config and
          silently skip the staged edits. */}
      <Button
        disabled={!canDeploy || hasStagedChanges}
        loading={deploy.isPending}
        onClick={() => deploy.mutate()}
        title={
          hasStagedChanges
            ? "You have unsaved changes — deploy them from the bar below"
            : undefined
        }
      >
        <RocketIcon />
        Deploy
      </Button>
      {hasStagedChanges ? (
        <p className="text-muted-foreground text-xs">Deploy staged changes from the bar below.</p>
      ) : error ? (
        <p className="text-destructive-foreground text-xs">{error}</p>
      ) : null}
    </div>
  );
}

function DeploymentsPanel({
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
          <h2 className="font-medium text-muted-foreground text-xs uppercase tracking-wider">
            History
          </h2>
          {latest ? (
            <span className="text-muted-foreground text-xs">
              Click a deployment for build logs and runtime data.
            </span>
          ) : null}
        </div>
        {isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : deployments.length === 0 ? (
          <p className="text-muted-foreground text-sm">No deployments yet.</p>
        ) : (
          <Card className="divide-y overflow-hidden p-0">
            {deployments.map((deployment) => (
              <div key={deployment.id} className="flex items-center gap-3 px-4 py-3">
                <button
                  className="-m-2 flex min-w-0 flex-1 items-center gap-3 rounded-md p-2 text-left transition hover:bg-muted/50 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
                  onClick={() => setSelectedDeploymentId(deployment.id)}
                  type="button"
                >
                  <StatusDot className="size-2.5" status={deployment.status} />
                  <span className="min-w-0 flex-1">
                    <span className="block truncate font-mono text-muted-foreground text-xs">
                      {new Date(deployment.createdAt).toLocaleString()}
                      {deployment.commitSha ? ` · ${deployment.commitSha.slice(0, 7)}` : ""}
                    </span>
                    <span className="block truncate text-muted-foreground/75 text-xs">
                      {deployment.imageRef ?? deployment.buildId ?? "No image recorded yet"}
                    </span>
                  </span>
                </button>
                <DeployStatusBadge size="sm" status={deployment.status} />
                <Button
                  disabled={
                    rollback.isPending ||
                    deployment.id === latest?.id ||
                    !deployment.imageRef ||
                    !["healthy", "superseded"].includes(deployment.status)
                  }
                  loading={rollback.isPending && rollback.variables === deployment.id}
                  onClick={() => rollback.mutate(deployment.id)}
                  size="sm"
                  title="Roll back to this deployment"
                  variant="outline"
                >
                  <RotateCcwIcon />
                  Rollback
                </Button>
              </div>
            ))}
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
    <div className="grid gap-3 text-sm sm:grid-cols-3">
      <div className="rounded-md border p-3">
        <p className="text-muted-foreground">Status</p>
        <div className="mt-2">
          <DeployStatusBadge status={deployment.status} />
        </div>
      </div>
      <div className="rounded-md border p-3">
        <p className="text-muted-foreground">Image</p>
        <p className="mt-2 truncate font-mono text-xs">
          {deployment.imageRef ?? "No image recorded"}
        </p>
      </div>
      <div className="rounded-md border p-3">
        <p className="text-muted-foreground">Updated</p>
        <p className="mt-2 font-mono text-xs">{new Date(deployment.updatedAt).toLocaleString()}</p>
      </div>
    </div>
  );
}

function DeploymentBuildLog({ deployment }: { deployment: Deployment }) {
  return (
    <section className="rounded-md border">
      <div className="flex items-center justify-between gap-3 border-b px-4 py-3">
        <h3 className="font-medium text-sm">Build logs</h3>
        <span className="font-mono text-muted-foreground text-xs">
          {deployment.buildId ?? deployment.id.slice(0, 8)}
        </span>
      </div>
      <pre className="max-h-80 overflow-auto bg-muted/35 p-4 font-mono text-xs leading-relaxed">
        {deployment.logs?.trim() ? deployment.logs : "Waiting for logs…"}
      </pre>
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
      <section className="rounded-md border p-4">
        <h3 className="font-medium text-sm">Runtime</h3>
        <p className="mt-2 text-muted-foreground text-sm">
          Attach a server before runtime metrics and logs are available.
        </p>
      </section>
    );
  }

  if (!runtimeAvailable) {
    return (
      <section className="rounded-md border p-4">
        <h3 className="font-medium text-sm">Runtime</h3>
        <p className="mt-2 text-muted-foreground text-sm">
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
    <section className="rounded-md border p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h3 className="font-medium text-sm">Runtime</h3>
          <p className="mt-1 text-muted-foreground text-sm">
            Live metrics and container logs for this deployment.
          </p>
        </div>
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

      <div className="mt-4 grid gap-3 text-sm sm:grid-cols-3">
        <UsageMetricTile
          label="CPU"
          value={metrics.data ? `${metrics.data.cpuPercent.toFixed(1)}%` : "Collecting"}
          detail={
            cpuLimitMillicores ? `Limit ${formatCpuCores(cpuLimitMillicores)}` : "No app CPU limit"
          }
          progress={cpuProgress}
        />
        <UsageMetricTile
          label="Memory"
          value={
            metrics.data
              ? `${formatBytes(metrics.data.memoryBytes)} / ${formatBytes(effectiveMemoryLimit)}`
              : "Collecting"
          }
          detail={
            app.resourceLimits.memoryBytes
              ? `Limit ${formatBytes(app.resourceLimits.memoryBytes)}`
              : "No app memory limit"
          }
          progress={memoryProgress}
        />
        <MetricTile
          label="Memory limit"
          value={effectiveMemoryLimit ? formatBytes(effectiveMemoryLimit) : "Collecting"}
        />
      </div>

      <div className="mt-4 h-64 overflow-hidden rounded-md border bg-muted/20 p-3">
        {samples.length > 1 ? (
          <LineChart
            animationDuration={700}
            aspectRatio={undefined}
            className="h-full"
            data={samples}
            margin={{ bottom: 28, left: 28, right: 20, top: 18 }}
            xDataKey="date"
          >
            <Grid horizontal />
            <Line dataKey="cpuPercent" stroke={chartCssVars.linePrimary} strokeWidth={2.5} />
            <Line dataKey="memoryPercent" stroke={chartCssVars.lineSecondary} strokeWidth={2.5} />
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
        ) : (
          <div className="flex h-full items-center justify-center text-muted-foreground text-sm">
            {metrics.isError ? "Metrics unavailable." : "Collecting metrics…"}
          </div>
        )}
      </div>
      <div className="mt-2 flex gap-4 text-muted-foreground text-xs">
        <span className="inline-flex items-center gap-1.5">
          <span className="size-2 rounded-full" style={{ background: chartCssVars.linePrimary }} />
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

      <div className="mt-5 border-t pt-5">
        <div className="flex items-center justify-between gap-3">
          <Label>Runtime logs</Label>
          <Button
            disabled={!serverId || logs.isFetching}
            onClick={() => void logs.refetch()}
            size="sm"
            type="button"
            variant="outline"
          >
            Refresh
          </Button>
        </div>
        <pre className="mt-2 max-h-72 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
          {logs.isError
            ? "Logs unavailable."
            : logs.data?.logs?.trim()
              ? logs.data.logs
              : "No logs yet."}
        </pre>
      </div>
    </section>
  );
}

function MetricTile({ label, value }: { label: string; value: string }) {
  return (
    <div className="rounded-md border bg-background p-3">
      <p className="text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-mono text-sm">{value}</p>
    </div>
  );
}

function UsageMetricTile({
  label,
  value,
  detail,
  progress,
}: {
  label: string;
  value: string;
  detail: string;
  progress: number | null;
}) {
  return (
    <div className="rounded-md border bg-background p-3">
      <p className="text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-mono text-sm">{value}</p>
      <div className="mt-2 h-1.5 overflow-hidden rounded-full bg-muted">
        <div
          className="h-full rounded-full bg-primary transition-all"
          style={{ width: `${progress ?? 0}%` }}
        />
      </div>
      <p className="mt-1 truncate text-muted-foreground text-xs">{detail}</p>
    </div>
  );
}

function formatCpuCores(millicores: number): string {
  const cores = millicores / 1000;
  return `${Number.isInteger(cores) ? cores.toFixed(0) : cores.toFixed(2)} CPU`;
}

function formatCpuInput(millicores: number): string {
  const cores = millicores / 1000;
  return Number.isInteger(cores) ? cores.toFixed(0) : cores.toFixed(2);
}

function sliderToCpuInput(millicores: number): string {
  return millicores <= 0 ? "" : formatCpuInput(millicores);
}

function sliderToMemoryInput(megabytes: number): string {
  return megabytes <= 0 ? "" : String(megabytes);
}

function parseCpuLimit(value: string): number | null | undefined {
  if (!value.trim()) return null;
  const cores = Number(value);
  if (!Number.isFinite(cores) || cores <= 0) return undefined;
  return Math.round(cores * 1000);
}

function parseMemoryLimit(value: string): number | null | undefined {
  if (!value.trim()) return null;
  const megabytes = Number(value);
  if (!Number.isFinite(megabytes) || megabytes <= 0) return undefined;
  return Math.round(megabytes * 1024 * 1024);
}

function DatabaseConnectionCard({ app }: { app: App }) {
  const { copiedId, copy } = useClipboard();
  const connection = useQuery({
    queryKey: ["database-connection", app.id],
    queryFn: () => getDatabaseConnectionInfo(app.id),
    enabled: app.appKind === "database",
  });

  const internalUri = connection.data?.internalUri ?? "";
  const publicUri = connection.data?.publicUri ?? "";

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Connection</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Use the internal URI from other Basse apps on the same server.
      </p>
      <div className="mt-4 flex flex-col gap-4">
        <ConnectionValue
          copied={copiedId === "internal"}
          label="Internal URI"
          loading={connection.isPending}
          onCopy={() => copy("internal", internalUri)}
          value={internalUri}
        />
        {app.database?.publicEnabled ? (
          <ConnectionValue
            copied={copiedId === "public"}
            label="Public URI"
            loading={connection.isPending}
            onCopy={() => copy("public", publicUri)}
            value={publicUri || "Redeploy after enabling public access."}
          />
        ) : null}
      </div>
      {connection.isError ? (
        <p className="mt-3 text-destructive-foreground text-sm">{connection.error.message}</p>
      ) : null}
    </Card>
  );
}

function ConnectionValue({
  copied,
  label,
  loading,
  onCopy,
  value,
}: {
  copied: boolean;
  label: string;
  loading: boolean;
  onCopy: () => void;
  value: string;
}) {
  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      <div className="flex min-w-0 items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs">
          {loading ? "Loading..." : value}
        </code>
        <Button disabled={!value || loading} onClick={onCopy} size="icon" variant="outline">
          {copied ? <CheckIcon /> : <CopyIcon />}
        </Button>
      </div>
    </div>
  );
}

function DatabaseSettingsCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const database = app.database;
  const databaseKind = database?.kind ?? "postgres";
  const engineLabel = databaseEngineLabel(databaseKind);
  const [version, setVersion] = useState(
    database?.version ?? (databaseKind === "redis" ? "8" : "18"),
  );
  const [publicEnabled, setPublicEnabled] = useState(database?.publicEnabled ?? false);
  const [publicPort, setPublicPort] = useState(
    String(database?.publicPort ?? databaseDefaultPort(databaseKind)),
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setVersion(database?.version ?? (databaseKind === "redis" ? "8" : "18"));
    setPublicEnabled(database?.publicEnabled ?? false);
    setPublicPort(String(database?.publicPort ?? databaseDefaultPort(databaseKind)));
  }, [database?.version, database?.publicEnabled, database?.publicPort, databaseKind]);

  const update = useMutation({
    mutationFn: () =>
      stageAppChanges(app.id, {
        databaseVersion: version,
        databasePublicEnabled: publicEnabled,
        databasePublicPort: publicEnabled ? Number(publicPort) : null,
      }),
    onSuccess: (data) => {
      setError(null);
      toast.success("Database change staged");
      queryClient.setQueryData(["changes", app.id], data);
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <Card className="p-6">
      <div className="flex items-center gap-2">
        <DatabaseIcon className="size-6" kind={databaseKind} />
        <h2 className="font-semibold text-lg">Database</h2>
      </div>
      <p className="mt-1 text-muted-foreground text-sm">
        Managed standalone {engineLabel} settings.
      </p>
      <form
        className="mt-4 space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          update.mutate();
        }}
      >
        <div className="grid gap-3 sm:grid-cols-[1fr_140px]">
          <div className="space-y-2">
            <Label htmlFor="database-version">{engineLabel} version</Label>
            <Input
              id="database-version"
              onChange={(event) => setVersion(event.currentTarget.value)}
              value={version}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="database-public-port">Public port</Label>
            <Input
              disabled={!publicEnabled}
              id="database-public-port"
              onChange={(event) => setPublicPort(event.currentTarget.value)}
              type="number"
              value={publicPort}
            />
          </div>
        </div>
        <label className="flex items-center gap-2 text-sm">
          <Checkbox
            checked={publicEnabled}
            onCheckedChange={(value) => setPublicEnabled(value === true)}
          />
          <span>Enable public TCP access</span>
        </label>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button loading={update.isPending} type="submit">
          Stage database changes
        </Button>
      </form>
    </Card>
  );
}

function ServerCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const servers = useQuery({ queryKey: ["servers", "for-apps"], queryFn: listServers });

  const setServers = useMutation({
    mutationFn: (serverIds: string[]) => stageAppChanges(app.id, { serverIds }),
    onSuccess: (data) => {
      toast.success("Server change staged");
      queryClient.setQueryData(["changes", app.id], data);
    },
    onError: (error: Error) =>
      toast.error("Couldn't stage servers", { description: toMessage(error) }),
  });

  const serverList = servers.data ?? [];
  const selectedServerIds = app.serverIds;
  const databaseApp = app.appKind === "database";

  function toggleServer(serverId: string, checked: boolean) {
    const next = databaseApp
      ? checked
        ? [serverId]
        : []
      : checked
        ? [...new Set([...selectedServerIds, serverId])]
        : selectedServerIds.filter((id) => id !== serverId);
    setServers.mutate(next);
  }

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Servers</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        {databaseApp
          ? "The server this standalone database deploys to."
          : "The servers this app deploys to. Only active servers can run deployments."}
      </p>
      <div className="mt-4">
        {servers.isPending ? (
          <p className="text-muted-foreground text-sm">Loading servers…</p>
        ) : serverList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No servers in this workspace yet.</p>
        ) : (
          <div className="flex flex-col gap-2">
            {serverList.map((s) => {
              const checked = selectedServerIds.includes(s.id);
              return (
                <label
                  key={s.id}
                  className="flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-sm"
                >
                  <span className="min-w-0">
                    <span className="block truncate font-medium">{s.name}</span>
                    <span className="text-muted-foreground">{s.status}</span>
                  </span>
                  <Checkbox
                    checked={checked}
                    disabled={setServers.isPending}
                    onCheckedChange={(value) => toggleServer(s.id, value === true)}
                  />
                </label>
              );
            })}
          </div>
        )}
      </div>
    </Card>
  );
}

function BuildSettingsCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const [sourceType, setSourceType] = useState<AppSourceType>(app.sourceType);
  const [repositoryUrl, setRepositoryUrl] = useState(app.repositoryUrl);
  const [imageRef, setImageRef] = useState(app.imageRef ?? "");
  const [branch, setBranch] = useState(app.branch);
  const [port, setPort] = useState(String(app.port));
  const [error, setError] = useState<string | null>(null);

  // Re-seed only when the underlying draft values change, not on every draft
  // object rebuild — otherwise staging an unrelated field (e.g. the build
  // location below) would wipe text the user is still typing here.
  useEffect(() => {
    setSourceType(app.sourceType);
    setRepositoryUrl(app.repositoryUrl);
    setImageRef(app.imageRef ?? "");
    setBranch(app.branch);
    setPort(String(app.port));
  }, [app.sourceType, app.repositoryUrl, app.imageRef, app.branch, app.port]);

  const update = useMutation({
    mutationFn: (input: {
      sourceType?: AppSourceType;
      repositoryUrl?: string;
      imageRef?: string | null;
      branch?: string;
      port?: number;
      buildRunner?: AppBuildRunner;
    }) => stageAppChanges(app.id, input),
    onSuccess: (data) => {
      setError(null);
      toast.success("Build change staged");
      queryClient.setQueryData(["changes", app.id], data);
    },
    onError: (e: Error) => setError(e.message),
  });
  const localBuildInvalid = app.buildRunner === "server" && app.serverIds.length !== 1;

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Build</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Choose whether this app builds from Git or deploys an existing image.
      </p>
      <form
        className="mt-4 space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          update.mutate({
            sourceType,
            repositoryUrl,
            imageRef: sourceType === "image" ? imageRef : null,
            branch,
            port: Number(port),
          });
        }}
      >
        <div className="space-y-2">
          <Label>Source</Label>
          <Select
            value={sourceType}
            onValueChange={(value) => setSourceType((value ?? "repository") as AppSourceType)}
          >
            <SelectTrigger className="w-full">
              <SelectValue placeholder="Source">
                {(value: AppSourceType) =>
                  value === "image" ? "Prebuilt Docker image" : "Git repository"
                }
              </SelectValue>
            </SelectTrigger>
            <SelectPopup>
              <SelectItem value="repository">Git repository</SelectItem>
              <SelectItem value="image">Prebuilt Docker image</SelectItem>
            </SelectPopup>
          </Select>
        </div>
        {sourceType === "repository" ? (
          <>
            <div className="space-y-2">
              <Label htmlFor="app-source-repo">Repository URL</Label>
              <Input
                id="app-source-repo"
                onChange={(event) => setRepositoryUrl(event.currentTarget.value)}
                value={repositoryUrl}
              />
            </div>
            <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
              <div className="space-y-2">
                <Label htmlFor="app-source-branch">Branch</Label>
                <Input
                  id="app-source-branch"
                  onChange={(event) => setBranch(event.currentTarget.value)}
                  value={branch}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="app-source-port">Port</Label>
                <Input
                  id="app-source-port"
                  onChange={(event) => setPort(event.currentTarget.value)}
                  type="number"
                  value={port}
                />
              </div>
            </div>
          </>
        ) : (
          <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
            <div className="space-y-2">
              <Label htmlFor="app-source-image">Docker image</Label>
              <Input
                id="app-source-image"
                onChange={(event) => setImageRef(event.currentTarget.value)}
                placeholder="nginx:alpine"
                value={imageRef}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="app-image-port">Port</Label>
              <Input
                id="app-image-port"
                onChange={(event) => setPort(event.currentTarget.value)}
                type="number"
                value={port}
              />
            </div>
          </div>
        )}
        {sourceType === "repository" ? (
          <div className="space-y-2">
            <Label>Build location</Label>
            <Select
              value={app.buildRunner}
              onValueChange={(value) =>
                update.mutate({ buildRunner: (value ?? "depot") as AppBuildRunner })
              }
            >
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Build location">
                  {(value: AppBuildRunner) => (value === "server" ? "Selected server" : "Depot")}
                </SelectValue>
              </SelectTrigger>
              <SelectPopup>
                <SelectItem value="depot">Depot</SelectItem>
                <SelectItem value="server">Selected server</SelectItem>
              </SelectPopup>
            </Select>
            {localBuildInvalid ? (
              <p className="text-warning-foreground text-sm">
                Selected-server builds require exactly one server. Use Depot for multiple servers.
              </p>
            ) : null}
          </div>
        ) : null}
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button loading={update.isPending} type="submit">
          Stage build changes
        </Button>
      </form>
    </Card>
  );
}

function ResourceLimitsCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const [cpuCores, setCpuCores] = useState(
    app.resourceLimits.cpuMillicores ? formatCpuInput(app.resourceLimits.cpuMillicores) : "",
  );
  const [memoryMb, setMemoryMb] = useState(
    app.resourceLimits.memoryBytes
      ? String(Math.round(app.resourceLimits.memoryBytes / 1048576))
      : "",
  );
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setCpuCores(
      app.resourceLimits.cpuMillicores ? formatCpuInput(app.resourceLimits.cpuMillicores) : "",
    );
    setMemoryMb(
      app.resourceLimits.memoryBytes
        ? String(Math.round(app.resourceLimits.memoryBytes / 1048576))
        : "",
    );
  }, [app.resourceLimits.cpuMillicores, app.resourceLimits.memoryBytes]);

  const specs = useQuery({
    queryKey: ["app-resource-specs", app.id, app.serverIds],
    queryFn: async () =>
      Promise.all(
        app.serverIds.map(async (serverId) => {
          try {
            return { serverId, info: await getAgentInfo(serverId), error: null };
          } catch (fetchError) {
            return {
              serverId,
              info: null,
              error: fetchError instanceof Error ? fetchError.message : "Could not fetch specs",
            };
          }
        }),
      ),
    enabled: app.serverIds.length > 0,
  });

  const machineSpecs = (specs.data ?? []).flatMap((entry) =>
    entry.info?.docker
      ? [
          {
            serverId: entry.serverId,
            cpuMillicores: entry.info.docker.ncpu * 1000,
            memoryBytes: entry.info.docker.memTotal,
          },
        ]
      : [],
  );
  const cpuCap = machineSpecs.length
    ? Math.min(...machineSpecs.map((spec) => spec.cpuMillicores))
    : null;
  const memoryCap = machineSpecs.length
    ? Math.min(...machineSpecs.map((spec) => spec.memoryBytes))
    : null;
  const memoryCapMb = memoryCap ? Math.floor(memoryCap / 1048576) : null;
  const cpuSliderValue =
    cpuCores.trim() && Number.isFinite(Number(cpuCores)) ? Math.round(Number(cpuCores) * 1000) : 0;
  const memorySliderValue =
    memoryMb.trim() && Number.isFinite(Number(memoryMb)) ? Number(memoryMb) : 0;

  const save = useMutation({
    mutationFn: () => {
      const cpuLimitMillicores = parseCpuLimit(cpuCores);
      const memoryLimitBytes = parseMemoryLimit(memoryMb);

      if (cpuLimitMillicores === undefined) {
        throw new Error("CPU limit must be a number of cores.");
      }
      if (memoryLimitBytes === undefined) {
        throw new Error("Memory limit must be a number of MB.");
      }
      if (cpuCap && cpuLimitMillicores && cpuLimitMillicores > cpuCap) {
        throw new Error(`CPU limit cannot exceed ${formatCpuCores(cpuCap)} on the attached host.`);
      }
      if (memoryCap && memoryLimitBytes && memoryLimitBytes > memoryCap) {
        throw new Error(
          `Memory limit cannot exceed ${formatBytes(memoryCap)} on the attached host.`,
        );
      }

      return stageAppChanges(app.id, { cpuLimitMillicores, memoryLimitBytes });
    },
    onSuccess: (data) => {
      setError(null);
      toast.success("Resource limits staged");
      queryClient.setQueryData(["changes", app.id], data);
    },
    onError: (saveError: Error) => setError(saveError.message),
  });

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Resource limits</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Cap the container on the next deployment. Leave a field empty to use the host default.
      </p>

      <div className="mt-4 grid gap-3 text-sm sm:grid-cols-2">
        <div className="rounded-md border p-3">
          <p className="text-muted-foreground">Host CPU</p>
          <p className="mt-1 font-mono">
            {specs.isPending && app.serverIds.length > 0
              ? "Loading..."
              : cpuCap
                ? formatCpuCores(cpuCap)
                : "Unavailable"}
          </p>
        </div>
        <div className="rounded-md border p-3">
          <p className="text-muted-foreground">Host memory</p>
          <p className="mt-1 font-mono">
            {specs.isPending && app.serverIds.length > 0
              ? "Loading..."
              : memoryCap
                ? formatBytes(memoryCap)
                : "Unavailable"}
          </p>
        </div>
      </div>

      <form
        className="mt-4 space-y-4"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate();
        }}
      >
        <div className="space-y-5">
          <div className="space-y-2">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="resource-cpu">CPU cores</Label>
              <span className="font-mono text-muted-foreground text-sm">
                {cpuCores.trim() ? formatCpuCores(Math.round(Number(cpuCores) * 1000)) : "Unlimited"}
              </span>
            </div>
            {cpuCap ? (
              <Slider
                aria-label="CPU cores"
                max={cpuCap}
                min={0}
                onValueChange={(value) =>
                  setCpuCores(sliderToCpuInput(Array.isArray(value) ? (value[0] ?? 0) : value))
                }
                step={50}
                value={cpuSliderValue}
              />
            ) : (
              <Input
                id="resource-cpu"
                inputMode="decimal"
                min="0.05"
                onChange={(event) => setCpuCores(event.currentTarget.value)}
                placeholder="Unlimited"
                step="0.05"
                type="number"
                value={cpuCores}
              />
            )}
            <p className="text-muted-foreground text-xs">
              {cpuCap
                ? `Drag up to ${formatCpuCores(cpuCap)}. Slide to 0 for the host default.`
                : "Attach a server to cap CPU with a slider."}
            </p>
          </div>
          <div className="space-y-2">
            <div className="flex items-center justify-between gap-3">
              <Label htmlFor="resource-memory">Memory</Label>
              <span className="font-mono text-muted-foreground text-sm">
                {memoryMb.trim() ? formatBytes(Number(memoryMb) * 1048576) : "Unlimited"}
              </span>
            </div>
            {memoryCapMb ? (
              <Slider
                aria-label="Memory limit"
                max={memoryCapMb}
                min={0}
                onValueChange={(value) =>
                  setMemoryMb(sliderToMemoryInput(Array.isArray(value) ? (value[0] ?? 0) : value))
                }
                step={16}
                value={memorySliderValue}
              />
            ) : (
              <Input
                id="resource-memory"
                inputMode="numeric"
                min="16"
                onChange={(event) => setMemoryMb(event.currentTarget.value)}
                placeholder="Unlimited"
                step="16"
                type="number"
                value={memoryMb}
              />
            )}
            <p className="text-muted-foreground text-xs">
              {memoryCapMb
                ? `Drag up to ${formatBytes(memoryCapMb * 1048576)}. Slide to 0 for the host default.`
                : "Attach a server to cap memory with a slider."}
            </p>
          </div>
        </div>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button loading={save.isPending} type="submit">
          Stage resource limits
        </Button>
      </form>
    </Card>
  );
}

function VolumesCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const [volumes, setVolumes] = useState<AppVolume[]>(app.volumes);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    setVolumes(app.volumes.length > 0 ? app.volumes : []);
  }, [app.volumes]);

  const save = useMutation({
    mutationFn: () =>
      stageAppChanges(app.id, {
        volumes: volumes.filter((volume) => volume.hostPath.trim() || volume.containerPath.trim()),
      }),
    onSuccess: (data) => {
      setError(null);
      toast.success("Volumes staged");
      queryClient.setQueryData(["changes", app.id], data);
    },
    onError: (e: Error) => setError(e.message),
  });

  function updateVolume(index: number, patch: Partial<AppVolume>) {
    setVolumes((current) =>
      current.map((volume, currentIndex) =>
        currentIndex === index ? { ...volume, ...patch } : volume,
      ),
    );
  }

  function addVolume() {
    setVolumes((current) => [...current, { hostPath: "", containerPath: "", readOnly: false }]);
  }

  function removeVolume(index: number) {
    setVolumes((current) => current.filter((_, currentIndex) => currentIndex !== index));
  }

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Volumes</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Bind host paths into the container on every deployment.
      </p>
      <form
        className="mt-4 space-y-3"
        onSubmit={(event) => {
          event.preventDefault();
          save.mutate();
        }}
      >
        {volumes.length === 0 ? (
          <div className="rounded-md border border-dashed p-4 text-muted-foreground text-sm">
            No volumes configured.
          </div>
        ) : (
          <div className="flex flex-col gap-3">
            {volumes.map((volume, index) => (
              <div key={index} className="rounded-md border p-3">
                <div className="grid gap-3 sm:grid-cols-[1fr_1fr_auto]">
                  <div className="space-y-2">
                    <Label htmlFor={`volume-host-${index}`}>Host path</Label>
                    <Input
                      id={`volume-host-${index}`}
                      onChange={(event) =>
                        updateVolume(index, { hostPath: event.currentTarget.value })
                      }
                      placeholder="/srv/app/data"
                      value={volume.hostPath}
                    />
                  </div>
                  <div className="space-y-2">
                    <Label htmlFor={`volume-container-${index}`}>Container path</Label>
                    <Input
                      id={`volume-container-${index}`}
                      onChange={(event) =>
                        updateVolume(index, { containerPath: event.currentTarget.value })
                      }
                      placeholder="/data"
                      value={volume.containerPath}
                    />
                  </div>
                  <div className="flex items-end">
                    <Button
                      aria-label="Remove volume"
                      onClick={() => removeVolume(index)}
                      size="icon"
                      type="button"
                      variant="outline"
                    >
                      <TrashIcon className="size-4" />
                    </Button>
                  </div>
                </div>
                <label className="mt-3 flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={volume.readOnly}
                    onCheckedChange={(value) => updateVolume(index, { readOnly: value === true })}
                  />
                  <span>Read-only</span>
                </label>
              </div>
            ))}
          </div>
        )}
        <Button onClick={addVolume} type="button" variant="outline">
          <PlusIcon className="size-4" />
          Add volume
        </Button>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button loading={save.isPending} type="submit">
          Stage volume changes
        </Button>
      </form>
    </Card>
  );
}

function useClipboard() {
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

function EnvVarsCard({ appId, stagedChanges }: { appId: string; stagedChanges: StagedChange[] }) {
  const queryClient = useQueryClient();
  const maskedKey = ["env-vars", appId];
  const revealKey = ["env-vars-reveal", appId];
  const stagedEnvCount = stagedChanges.filter((change) => change.resource === "env_var").length;

  const [editing, setEditing] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const { copiedId, copy } = useClipboard();

  const vars = useQuery({ queryKey: maskedKey, queryFn: () => listEnvVars(appId) });
  const reveal = useQuery({
    queryKey: revealKey,
    queryFn: () => revealEnvVars(appId),
    enabled: revealed && !editing,
  });

  const list = vars.data ?? [];
  const revealedMap = new Map((reveal.data ?? []).map((v) => [v.key, v.value]));

  function ensureRevealed() {
    return queryClient.fetchQuery({ queryKey: revealKey, queryFn: () => revealEnvVars(appId) });
  }

  const save = useMutation({
    mutationFn: () => stageEnvVars(appId, { vars: parseDotenv(draft) }),
    onSuccess: (data) => {
      setError(null);
      setEditing(false);
      toast.success("Variables staged");
      queryClient.setQueryData(["changes", appId], data);
    },
    onError: (e: Error) => setError(e.message),
  });

  async function startEdit() {
    setError(null);
    setPreparing(true);
    try {
      // Seed from the draft (live env overlaid with staged edits) so the user
      // keeps building on what is already staged.
      setDraft(serializeDotenv(await getEnvDraft(appId)));
      setEditing(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load variables.");
    } finally {
      setPreparing(false);
    }
  }

  async function copyAll() {
    try {
      copy("__all__", serializeDotenv(await ensureRevealed()));
    } catch {
      // Reveal failed — nothing to copy.
    }
  }

  async function copyRow(key: string) {
    try {
      const pairs = await ensureRevealed();
      copy(key, pairs.find((pair) => pair.key === key)?.value ?? "");
    } catch {
      // Reveal failed — nothing to copy.
    }
  }

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Environment variables</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Runtime variables, encrypted at rest. Edits are staged until you deploy.
          </p>
          {stagedEnvCount > 0 ? (
            <p className="mt-1 text-primary text-sm">
              {stagedEnvCount} variable change{stagedEnvCount === 1 ? "" : "s"} staged — review in
              the bar below.
            </p>
          ) : null}
        </div>
        {!editing && list.length > 0 ? (
          <div className="flex items-center gap-2">
            <Button onClick={() => setRevealed((value) => !value)} size="sm" variant="outline">
              {revealed ? <EyeOffIcon /> : <EyeIcon />}
              {revealed ? "Hide" : "Reveal"}
            </Button>
            <Button onClick={copyAll} size="sm" variant="outline">
              {copiedId === "__all__" ? <CheckIcon /> : <CopyIcon />}
              .env
            </Button>
            <Button loading={preparing} onClick={startEdit} size="sm">
              <PencilIcon />
              Edit
            </Button>
          </div>
        ) : null}
      </div>

      {editing ? (
        <form
          className="mt-5 space-y-3"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate();
          }}
        >
          <Textarea
            autoFocus
            className="min-h-56 font-mono text-xs leading-relaxed"
            onChange={(event) => setDraft(event.currentTarget.value)}
            spellCheck={false}
            value={draft}
          />
          <p className="text-muted-foreground text-xs">
            One <code className="font-mono">KEY=value</code> per line. Quote values with spaces, use{" "}
            <code className="font-mono">\n</code> for newlines, <code className="font-mono">#</code>{" "}
            for comments. Staging replaces the whole set; deploy from the bar to apply.
          </p>
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          <div className="flex gap-2">
            <Button loading={save.isPending} type="submit">
              Stage variables
            </Button>
            <Button
              onClick={() => {
                setEditing(false);
                setError(null);
              }}
              type="button"
              variant="outline"
            >
              Cancel
            </Button>
          </div>
        </form>
      ) : vars.isPending ? (
        <p className="mt-5 text-muted-foreground text-sm">Loading…</p>
      ) : list.length === 0 ? (
        <div className="mt-5 flex flex-col items-start gap-3 rounded-md border border-dashed p-5">
          <p className="text-muted-foreground text-sm">No variables set.</p>
          <Button loading={preparing} onClick={startEdit} size="sm" variant="outline">
            <PlusIcon />
            Add variables
          </Button>
        </div>
      ) : (
        <ul className="mt-5 divide-y rounded-md border">
          {list.map((v) => {
            const display = revealed
              ? reveal.isPending
                ? "…"
                : (revealedMap.get(v.key) ?? "")
              : v.valueHint;
            return (
              <li key={v.key} className="flex items-center gap-3 px-3 py-2 font-mono text-sm">
                <span className="min-w-0 max-w-[45%] truncate font-medium text-foreground">
                  {v.key}
                </span>
                <span
                  className="min-w-0 flex-1 truncate text-right text-muted-foreground"
                  title={revealed ? revealedMap.get(v.key) : undefined}
                >
                  {display}
                </span>
                <Button
                  aria-label={`Copy ${v.key}`}
                  className="shrink-0"
                  onClick={() => copyRow(v.key)}
                  size="icon-xs"
                  variant="ghost"
                >
                  {copiedId === v.key ? <CheckIcon /> : <CopyIcon />}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}

function AppDomainsSection({ app, serverId }: { app: App; serverId: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["domains", serverId];
  const upstream = `basse-app-${app.id}:${app.port}`;
  const servers = useQuery({ queryKey: ["servers", "for-domains"], queryFn: listServers });
  const [host, setHost] = useState("");
  const [error, setError] = useState<string | null>(null);

  const domains = useQuery({
    queryKey,
    queryFn: () => listDomains(serverId),
    refetchInterval: (query) =>
      (query.state.data ?? []).some((d) => d.status === "pending") ? 2000 : false,
  });

  const add = useMutation({
    mutationFn: () => createDomain(serverId, { host, upstream, appId: app.id }),
    onSuccess: async () => {
      setHost("");
      setError(null);
      toast.success("Domain added");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteDomain(id),
    onSuccess: async () => {
      toast.success("Domain removed");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => toast.error("Couldn't remove domain", { description: toMessage(e) }),
  });

  const addPreview = useMutation({
    mutationFn: () => {
      const ip = selectedServer?.sshHost ?? "";
      const previewDomainHost = `${app.slug}-${app.id.slice(0, 8)}.${ip}.sslip.io`;
      return createDomain(serverId, { host: previewDomainHost, upstream, appId: app.id });
    },
    onSuccess: async () => {
      setError(null);
      toast.success("Preview domain created");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  // Only this app's domains (the server may host domains for other apps too).
  const appDomains = (domains.data ?? []).filter((d) => d.appId === app.id);
  const selectedServer = (servers.data ?? []).find((s) => s.id === serverId);
  const canGeneratePreview = Boolean(selectedServer?.sshHost.match(/^\d{1,3}(?:\.\d{1,3}){3}$/));
  const previewHost = selectedServer
    ? `${app.slug}-${app.id.slice(0, 8)}.${selectedServer.sshHost}.sslip.io`
    : "";

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Domains</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Add an A record for your domain pointing to{" "}
        <code className="font-mono">{selectedServer?.sshHost ?? "this server"}</code>. Basse will
        configure HTTPS on that server and route traffic to the app.
      </p>
      <div className="mt-4 rounded-md border bg-muted/20 p-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="font-medium text-sm">Preview domain</p>
            <p className="truncate font-mono text-muted-foreground text-xs">
              {previewHost || "Requires an IPv4 server address"}
            </p>
          </div>
          <Button
            disabled={!canGeneratePreview}
            loading={addPreview.isPending}
            onClick={() => addPreview.mutate()}
            size="sm"
            variant="outline"
          >
            Generate sslip.io
          </Button>
        </div>
      </div>

      <div className="mt-5">
        {appDomains.length === 0 ? (
          <p className="text-muted-foreground text-sm">No domains yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {appDomains.map((d) => (
              <li
                key={d.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
              >
                <span className="flex min-w-0 items-center gap-2">
                  <Badge
                    size="sm"
                    variant={
                      d.status === "active" ? "success" : d.status === "error" ? "error" : "warning"
                    }
                  >
                    {d.status}
                  </Badge>
                  <span className="truncate font-medium text-sm">{d.host}</span>
                </span>
                <Button
                  aria-label={`Delete ${d.host}`}
                  loading={remove.isPending && remove.variables === d.id}
                  onClick={() => remove.mutate(d.id)}
                  size="icon"
                  variant="outline"
                >
                  <TrashIcon />
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>

      <form className="mt-6 flex items-end gap-2 border-t pt-6" onSubmit={handleSubmit}>
        <div className="flex-1 space-y-2">
          <Label htmlFor="app-domain-host">Domain</Label>
          <Input
            id="app-domain-host"
            onChange={(e) => setHost(e.currentTarget.value)}
            placeholder="app.example.com"
            required
            value={host}
          />
        </div>
        <Button loading={add.isPending} type="submit">
          Add
        </Button>
      </form>
      {error ? <p className="mt-2 text-destructive-foreground text-sm">{error}</p> : null}
    </Card>
  );
}

const LOAD_BALANCER_STATUS_VARIANT = {
  pending: "outline",
  syncing: "warning",
  active: "success",
  error: "error",
} as const;

function ManagedLoadBalancerSection({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const integrationsKey = ["load-balancer-integrations"];
  const loadBalancersKey = ["load-balancers", app.id];
  const integrations = useQuery({
    queryKey: integrationsKey,
    queryFn: listLoadBalancerIntegrations,
  });
  const loadBalancers = useQuery({
    queryKey: loadBalancersKey,
    queryFn: () => listManagedLoadBalancers(app.id),
  });
  const [integrationId, setIntegrationId] = useState("");
  const [host, setHost] = useState("");
  const [location, setLocation] = useState("fsn1");
  const [loadBalancerType, setLoadBalancerType] = useState("lb11");
  const [healthCheckPath, setHealthCheckPath] = useState("/");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    const firstIntegration = integrations.data?.[0]?.id ?? "";
    if (!integrationId && firstIntegration) {
      setIntegrationId(firstIntegration);
    }
  }, [integrationId, integrations.data]);

  const create = useMutation({
    mutationFn: () =>
      createManagedLoadBalancer({
        appId: app.id,
        integrationId,
        host,
        location,
        loadBalancerType,
        healthCheckPath,
      }),
    onSuccess: async (created) => {
      setError(null);
      setHost("");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: loadBalancersKey }),
        ...app.serverIds.map((serverId) =>
          queryClient.invalidateQueries({ queryKey: ["domains", serverId] }),
        ),
      ]);
      if (created.status === "error") {
        toast.error("Load balancer saved but sync failed", {
          description: created.statusMessage ?? "Open the load balancer card and sync again.",
        });
      } else {
        toast.success("Load balancer created");
      }
    },
    onError: (createError: Error) => setError(createError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    create.mutate();
  }

  const integrationList = integrations.data ?? [];
  const existing = loadBalancers.data ?? [];

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Managed load balancer</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Basse keeps the same domain route on every attached server, then points a provider load
            balancer at those servers for 80/443 traffic.
          </p>
        </div>
        <Badge variant="outline">{app.serverIds.length} targets</Badge>
      </div>

      {integrations.isPending || loadBalancers.isPending ? (
        <p className="mt-5 text-muted-foreground text-sm">Loading…</p>
      ) : integrationList.length === 0 ? (
        <div className="mt-5 rounded-md border border-dashed p-5">
          <p className="text-muted-foreground text-sm">
            Connect Hetzner in Settings before creating a managed load balancer.
          </p>
          <Button className="mt-3" render={<Link to="/settings" />} size="sm" variant="outline">
            Open settings
          </Button>
        </div>
      ) : existing.length > 0 ? (
        <div className="mt-5 flex flex-col gap-3">
          {existing.map((loadBalancer) => (
            <ManagedLoadBalancerCard
              app={app}
              key={loadBalancer.id}
              loadBalancer={loadBalancer}
              queryKey={loadBalancersKey}
            />
          ))}
        </div>
      ) : (
        <form className="mt-5 space-y-4" onSubmit={handleSubmit}>
          <div className="grid gap-3 sm:grid-cols-[1fr_150px]">
            <div className="space-y-2">
              <Label htmlFor="load-balancer-host">Domain</Label>
              <Input
                id="load-balancer-host"
                onChange={(event) => setHost(event.currentTarget.value)}
                placeholder="app.example.com"
                required
                value={host}
              />
            </div>
            <div className="space-y-2">
              <Label>Provider</Label>
              <Select value={integrationId} onValueChange={(value) => setIntegrationId(value ?? "")}>
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Provider">
                    {(value: string) =>
                      integrationList.find((integration) => integration.id === value)?.name ??
                      "Provider"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  {integrationList.map((integration) => (
                    <SelectItem key={integration.id} value={integration.id}>
                      {integration.name}
                    </SelectItem>
                  ))}
                </SelectPopup>
              </Select>
            </div>
          </div>
          <div className="grid gap-3 sm:grid-cols-3">
            <div className="space-y-2">
              <Label htmlFor="load-balancer-location">Hetzner location</Label>
              <Input
                id="load-balancer-location"
                onChange={(event) => setLocation(event.currentTarget.value)}
                value={location}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="load-balancer-type">Type</Label>
              <Input
                id="load-balancer-type"
                onChange={(event) => setLoadBalancerType(event.currentTarget.value)}
                value={loadBalancerType}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="load-balancer-health">Health path</Label>
              <Input
                id="load-balancer-health"
                onChange={(event) => setHealthCheckPath(event.currentTarget.value)}
                value={healthCheckPath}
              />
            </div>
          </div>
          <p className="text-muted-foreground text-xs">
            Hetzner v0 creates a Basse-owned load balancer with TCP passthrough on 80 and 443, so
            each target server's Caddy keeps handling TLS and app routing.
          </p>
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          <Button disabled={!integrationId || !host.trim()} loading={create.isPending} type="submit">
            <PlusIcon />
            Create load balancer
          </Button>
        </form>
      )}
    </Card>
  );
}

function ManagedLoadBalancerCard({
  app,
  loadBalancer,
  queryKey,
}: {
  app: App;
  loadBalancer: ManagedLoadBalancer;
  queryKey: unknown[];
}) {
  const queryClient = useQueryClient();
  const sync = useMutation({
    mutationFn: () => syncManagedLoadBalancer(loadBalancer.id),
    onSuccess: async (updated) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        ...app.serverIds.map((serverId) =>
          queryClient.invalidateQueries({ queryKey: ["domains", serverId] }),
        ),
      ]);
      if (updated.status === "error") {
        toast.error("Load balancer sync failed", {
          description: updated.statusMessage ?? "Check provider settings and server targets.",
        });
      } else {
        toast.success("Load balancer synced");
      }
    },
    onError: (syncError: Error) =>
      toast.error("Couldn't sync load balancer", { description: syncError.message }),
  });
  const remove = useMutation({
    mutationFn: () => deleteManagedLoadBalancer(loadBalancer.id),
    onSuccess: async () => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        ...app.serverIds.map((serverId) =>
          queryClient.invalidateQueries({ queryKey: ["domains", serverId] }),
        ),
      ]);
      toast.success("Load balancer deleted");
    },
    onError: (removeError: Error) =>
      toast.error("Couldn't delete load balancer", { description: removeError.message }),
  });

  function confirmDelete() {
    if (!window.confirm(`Delete ${loadBalancer.name} and its provider resource?`)) return;
    remove.mutate();
  }

  return (
    <div className="rounded-md border p-4">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h3 className="truncate font-medium">{loadBalancer.host}</h3>
            <Badge variant={LOAD_BALANCER_STATUS_VARIANT[loadBalancer.status]}>
              {loadBalancer.status}
            </Badge>
          </div>
          <p className="mt-1 truncate font-mono text-muted-foreground text-xs">
            {loadBalancer.provider} · {loadBalancer.loadBalancerType} · {loadBalancer.location}
          </p>
        </div>
        <div className="flex items-center gap-2">
          <Button
            loading={sync.isPending}
            onClick={() => sync.mutate()}
            size="sm"
            variant="outline"
          >
            <RotateCcwIcon />
            Sync
          </Button>
          <Button
            aria-label={`Delete ${loadBalancer.host}`}
            loading={remove.isPending}
            onClick={confirmDelete}
            size="icon"
            variant="outline"
          >
            <TrashIcon />
          </Button>
        </div>
      </div>

      {loadBalancer.statusMessage ? (
        <p className="mt-3 text-destructive-foreground text-sm">{loadBalancer.statusMessage}</p>
      ) : null}

      <div className="mt-4 grid gap-3 text-sm sm:grid-cols-2">
        <DnsRecord label="A record" value={loadBalancer.endpointIpv4} host={loadBalancer.host} />
        <DnsRecord label="AAAA record" value={loadBalancer.endpointIpv6} host={loadBalancer.host} />
      </div>

      <div className="mt-4">
        <p className="font-medium text-sm">Targets</p>
        {loadBalancer.targets.length === 0 ? (
          <p className="mt-1 text-muted-foreground text-sm">No targets synced yet.</p>
        ) : (
          <ul className="mt-2 flex flex-col gap-2">
            {loadBalancer.targets.map((target) => (
              <li
                key={target.id}
                className="flex items-center justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-sm"
              >
                <span className="min-w-0 truncate font-mono text-xs">{target.address}</span>
                <Badge
                  size="sm"
                  variant={
                    target.status === "active"
                      ? "success"
                      : target.status === "error"
                        ? "error"
                        : "outline"
                  }
                >
                  {target.status}
                </Badge>
              </li>
            ))}
          </ul>
        )}
      </div>

      {loadBalancer.lastSyncedAt ? (
        <p className="mt-4 text-muted-foreground text-xs">
          Last synced {new Date(loadBalancer.lastSyncedAt).toLocaleString()}
        </p>
      ) : null}
    </div>
  );
}

function DnsRecord({
  label,
  value,
  host,
}: {
  label: string;
  value: string | null;
  host: string;
}) {
  return (
    <div className="rounded-md border bg-background p-3">
      <p className="text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-mono text-xs">
        {value ? `${host} -> ${value}` : "Waiting for provider endpoint"}
      </p>
    </div>
  );
}
