import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
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
import type { AppBuildRunner, AppSourceType, AppVolume, Deployment } from "@basse/shared";
import { chartCssVars } from "@/components/charts/chart-context";
import { Grid } from "@/components/charts/grid";
import { Line, LineChart } from "@/components/charts/line-chart";
import { ChartTooltip } from "@/components/charts/tooltip";
import { XAxis } from "@/components/charts/x-axis";
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
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import { Textarea } from "@/components/ui/textarea";
import type { App } from "@/lib/apps";
import {
  getApp,
  getAppLogs,
  getAppMetrics,
  deleteApp,
  runAppConsoleCommand,
  stopAppContainer,
  updateApp,
} from "@/lib/apps";
import { listDeployments, rollbackDeployment, triggerDeploy } from "@/lib/deployments";
import { createDomain, deleteDomain, listDomains } from "@/lib/domains";
import { parseDotenv, serializeDotenv } from "@/lib/dotenv";
import { listEnvVars, revealEnvVars, setEnvVars } from "@/lib/env-vars";
import { listServers } from "@/lib/servers";

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

  if (app.isPending) {
    return <p className="p-4 text-muted-foreground text-sm md:p-6">Loading…</p>;
  }
  if (app.isError || !app.data) {
    return <p className="p-4 text-destructive-foreground text-sm md:p-6">App not found.</p>;
  }

  const data = app.data;
  const list = deployments.data ?? [];
  const status = list[0]?.status ?? data.latestDeploymentStatus ?? null;
  const canDeploy =
    data.serverIds.length > 0 &&
    (data.sourceType === "image" || data.buildRunner !== "server" || data.serverIds.length === 1);

  return (
    <section className="flex flex-1 flex-col p-4 md:p-6">
      <div className="mx-auto flex w-full max-w-4xl flex-col gap-6">
        <Breadcrumb app={data} />
        <AppHeader app={data} appId={appId} canDeploy={canDeploy} status={status} />

        <Tabs defaultValue="deployments">
          <TabsList variant="underline" className="w-full justify-start overflow-x-auto">
            <TabsTab value="deployments">Deployments</TabsTab>
            <TabsTab value="runtime">Runtime</TabsTab>
            <TabsTab value="variables">Variables</TabsTab>
            <TabsTab value="domains">Domains</TabsTab>
            <TabsTab value="settings">Settings</TabsTab>
          </TabsList>

          <TabsPanel className="pt-5" value="deployments">
            <DeploymentsPanel appId={appId} deployments={list} isPending={deployments.isPending} />
          </TabsPanel>
          <TabsPanel className="pt-5" value="runtime">
            <RuntimeCard app={data} />
          </TabsPanel>
          <TabsPanel className="pt-5" value="variables">
            <EnvVarsCard appId={appId} />
          </TabsPanel>
          <TabsPanel className="pt-5" value="domains">
            {data.serverIds.length === 1 ? (
              <AppDomainsSection app={data} serverId={data.serverIds[0]!} />
            ) : data.serverIds.length > 1 ? (
              <DisabledDomainsSection />
            ) : (
              <Card className="p-6">
                <p className="text-muted-foreground text-sm">
                  Attach a server to this app to route a domain to it.
                </p>
              </Card>
            )}
          </TabsPanel>
          <TabsPanel className="flex flex-col gap-6 pt-5" value="settings">
            <BuildSettingsCard app={data} />
            <ServerCard app={data} />
            <VolumesCard app={data} />
            <DeleteAppCard app={data} />
          </TabsPanel>
        </Tabs>
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
}: {
  app: App;
  appId: string;
  canDeploy: boolean;
  status: Deployment["status"] | null;
}) {
  const repoHost = app.repositoryUrl.replace(/^https?:\/\//, "");
  const liveUrl = useLiveUrl(app);

  return (
    <Card className="gap-5 p-6">
      <div className="flex flex-wrap items-start justify-between gap-4">
        <div className="flex min-w-0 items-center gap-3">
          <StatusDot className="size-3" status={status} />
          <h1 className="truncate font-semibold text-2xl tracking-tight">{app.name}</h1>
          <DeployStatusBadge status={status} />
        </div>
        <DeployButton appId={appId} canDeploy={canDeploy} />
      </div>

      <div className="flex flex-wrap items-center gap-x-2 gap-y-1 font-mono text-muted-foreground text-xs">
        {app.sourceType === "image" ? (
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
        <SpecDivider />
        {app.sourceType === "repository" ? (
          <>
            <span>{app.branch}</span>
            <SpecDivider />
          </>
        ) : null}
        <span>:{app.port}</span>
        {app.sourceType === "repository" ? (
          <>
            <SpecDivider />
            <span>{app.buildRunner}</span>
          </>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-3 border-t pt-4 text-sm">
        {liveUrl ? (
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

function DeployButton({ appId, canDeploy }: { appId: string; canDeploy: boolean }) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);

  const deploy = useMutation({
    mutationFn: () => triggerDeploy(appId),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey: ["deployments", appId] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <div className="flex flex-col items-end gap-1">
      <Button disabled={!canDeploy} loading={deploy.isPending} onClick={() => deploy.mutate()}>
        <RocketIcon />
        Deploy
      </Button>
      {error ? <p className="text-destructive-foreground text-xs">{error}</p> : null}
    </div>
  );
}

function DeploymentsPanel({
  appId,
  deployments,
  isPending,
}: {
  appId: string;
  deployments: Deployment[];
  isPending: boolean;
}) {
  const queryClient = useQueryClient();
  const latest = deployments[0];
  const [rollbackError, setRollbackError] = useState<string | null>(null);

  const rollback = useMutation({
    mutationFn: (deploymentId: string) => rollbackDeployment(deploymentId),
    onSuccess: async () => {
      setRollbackError(null);
      await queryClient.invalidateQueries({ queryKey: ["deployments", appId] });
      await queryClient.invalidateQueries({ queryKey: ["app", appId] });
    },
    onError: (error: Error) => setRollbackError(error.message),
  });

  return (
    <div className="flex flex-col gap-5">
      <Card className="overflow-hidden p-0">
        <div className="flex items-center justify-between gap-3 border-b px-5 py-3">
          <h2 className="font-medium text-sm">Latest build log</h2>
          {latest ? <DeployStatusBadge size="sm" status={latest.status} /> : null}
        </div>
        <pre className="max-h-80 overflow-auto bg-muted/30 p-4 font-mono text-xs leading-relaxed">
          {latest ? (latest.logs ?? "Waiting for logs…") : "No deployments yet."}
        </pre>
      </Card>

      <div>
        <h2 className="mb-2 font-medium text-muted-foreground text-xs uppercase tracking-wider">
          History
        </h2>
        {isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : deployments.length === 0 ? (
          <p className="text-muted-foreground text-sm">No deployments yet.</p>
        ) : (
          <Card className="divide-y overflow-hidden p-0">
            {deployments.map((deployment) => (
              <div key={deployment.id} className="flex items-center gap-3 px-4 py-3">
                <StatusDot status={deployment.status} />
                <span className="flex-1 font-mono text-muted-foreground text-xs">
                  {new Date(deployment.createdAt).toLocaleString()}
                  {deployment.commitSha ? ` · ${deployment.commitSha.slice(0, 7)}` : ""}
                </span>
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
    </div>
  );
}

function ServerCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const servers = useQuery({ queryKey: ["servers", "for-apps"], queryFn: listServers });

  const setServers = useMutation({
    mutationFn: (serverIds: string[]) => updateApp(app.id, { serverIds }),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["app", app.id] });
    },
  });

  const serverList = servers.data ?? [];
  const selectedServerIds = app.serverIds;

  function toggleServer(serverId: string, checked: boolean) {
    const next = checked
      ? [...new Set([...selectedServerIds, serverId])]
      : selectedServerIds.filter((id) => id !== serverId);
    setServers.mutate(next);
  }

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Servers</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        The servers this app deploys to. Only active servers can run deployments.
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

  useEffect(() => {
    setSourceType(app.sourceType);
    setRepositoryUrl(app.repositoryUrl);
    setImageRef(app.imageRef ?? "");
    setBranch(app.branch);
    setPort(String(app.port));
  }, [app]);

  const update = useMutation({
    mutationFn: (input: {
      sourceType?: AppSourceType;
      repositoryUrl?: string;
      imageRef?: string | null;
      branch?: string;
      port?: number;
      buildRunner?: AppBuildRunner;
    }) => updateApp(app.id, input),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey: ["app", app.id] });
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
          Save build settings
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
      updateApp(app.id, {
        volumes: volumes.filter((volume) => volume.hostPath.trim() || volume.containerPath.trim()),
      }),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey: ["app", app.id] });
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
          Save volumes
        </Button>
      </form>
    </Card>
  );
}

function RuntimeCard({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const servers = useQuery({ queryKey: ["servers", "runtime"], queryFn: listServers });
  const targetServers = (servers.data ?? []).filter((server) => app.serverIds.includes(server.id));
  const [serverId, setServerId] = useState(app.serverIds[0] ?? "");
  const [stopError, setStopError] = useState<string | null>(null);
  const [samples, setSamples] = useState<
    { date: Date; cpuPercent: number; memoryPercent: number }[]
  >([]);

  useEffect(() => {
    if (!serverId || !app.serverIds.includes(serverId)) {
      setServerId(app.serverIds[0] ?? "");
      setSamples([]);
    }
  }, [app.serverIds, serverId]);

  const metrics = useQuery({
    queryKey: ["app-metrics", app.id, serverId],
    queryFn: () => getAppMetrics(app.id, serverId),
    enabled: Boolean(serverId),
    refetchInterval: 5000,
  });

  const logs = useQuery({
    queryKey: ["app-logs", app.id, serverId],
    queryFn: () => getAppLogs(app.id, serverId),
    enabled: Boolean(serverId),
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
      },
    ]);
  }, [metrics.data]);

  const stop = useMutation({
    mutationFn: () => stopAppContainer(app.id, { serverId }),
    onSuccess: async () => {
      setStopError(null);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["app-metrics", app.id, serverId] }),
        queryClient.invalidateQueries({ queryKey: ["app-logs", app.id, serverId] }),
        queryClient.invalidateQueries({ queryKey: ["deployments", app.id] }),
      ]);
    },
    onError: (error: Error) => setStopError(error.message),
  });

  return (
    <Card className="p-6">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Runtime</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Live container metrics, logs, and command console for the selected server.
          </p>
        </div>
        <div className="flex items-center gap-2">
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
          <Button
            disabled={!serverId}
            loading={stop.isPending}
            onClick={() => stop.mutate()}
            size="sm"
            variant="destructive-outline"
          >
            Stop
          </Button>
        </div>
      </div>
      {stopError ? <p className="mt-2 text-destructive-foreground text-sm">{stopError}</p> : null}

      {app.serverIds.length === 0 ? (
        <p className="mt-5 text-muted-foreground text-sm">
          Select a server before using runtime tools.
        </p>
      ) : (
        <>
          <div className="mt-5 h-64 overflow-hidden rounded-md border bg-muted/20 p-3">
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
                <Line
                  dataKey="memoryPercent"
                  stroke={chartCssVars.lineSecondary}
                  strokeWidth={2.5}
                />
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
                      value: `${Number(point.memoryPercent).toFixed(1)}%`,
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

          <div className="mt-6 border-t pt-6">
            <div className="flex items-center justify-between gap-3">
              <Label>Logs</Label>
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

          <AppConsoleTerminal appId={app.id} serverId={serverId} />
        </>
      )}
    </Card>
  );
}

function AppConsoleTerminal({ appId, serverId }: { appId: string; serverId: string }) {
  const containerRef = useRef<HTMLDivElement | null>(null);
  const terminalRef = useRef<Terminal | null>(null);
  const commandRef = useRef("");
  const runningRef = useRef(false);

  useEffect(() => {
    const container = containerRef.current;
    if (!container) return;

    commandRef.current = "";
    runningRef.current = false;

    const terminal = new Terminal({
      allowProposedApi: false,
      convertEol: true,
      cursorBlink: true,
      disableStdin: !serverId,
      fontFamily: '"Geist Mono Variable", ui-monospace, SFMono-Regular, Menlo, monospace',
      fontSize: 12,
      scrollback: 1000,
      theme: {
        background: "#0f1115",
        foreground: "#e5e7eb",
        cursor: "#e5e7eb",
        selectionBackground: "#334155",
      },
    });
    const fitAddon = new FitAddon();
    terminal.loadAddon(fitAddon);
    terminal.open(container);
    terminalRef.current = terminal;

    const fit = () => {
      try {
        fitAddon.fit();
      } catch {
        // xterm can throw if the container has not been measured yet.
      }
    };
    fit();
    const resizeObserver = new ResizeObserver(fit);
    resizeObserver.observe(container);

    const writePrompt = () => terminal.write("\r\n$ ");
    terminal.write(serverId ? "$ " : "Select a server first.");

    const dataDisposable = terminal.onData((data) => {
      if (!serverId || runningRef.current) return;

      if (data === "\r") {
        const command = commandRef.current.trim();
        terminal.write("\r\n");
        commandRef.current = "";
        if (!command) {
          terminal.write("$ ");
          return;
        }

        runningRef.current = true;
        void runAppConsoleCommand(appId, { command, serverId })
          .then((result) => {
            const output = result.output || "(no output)";
            terminal.write(output.endsWith("\n") ? output : `${output}\r\n`);
            terminal.write(`exit ${result.exitCode}`);
          })
          .catch((error: Error) => {
            terminal.write(`Error: ${error.message}`);
          })
          .finally(() => {
            runningRef.current = false;
            writePrompt();
          });
        return;
      }

      if (data === "\u007F") {
        if (commandRef.current.length > 0) {
          commandRef.current = commandRef.current.slice(0, -1);
          terminal.write("\b \b");
        }
        return;
      }

      if (data >= " " && data !== "\u007F") {
        commandRef.current += data;
        terminal.write(data);
      }
    });

    return () => {
      dataDisposable.dispose();
      resizeObserver.disconnect();
      terminal.dispose();
      terminalRef.current = null;
    };
  }, [appId, serverId]);

  return (
    <div className="mt-6 border-t pt-6">
      <Label>Console</Label>
      <div className="mt-2 overflow-hidden rounded-md border bg-[#0f1115] p-2">
        <div ref={containerRef} className="h-72" />
      </div>
    </div>
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

function EnvVarsCard({ appId }: { appId: string }) {
  const queryClient = useQueryClient();
  const maskedKey = ["env-vars", appId];
  const revealKey = ["env-vars-reveal", appId];

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
    mutationFn: () => setEnvVars(appId, parseDotenv(draft)),
    onSuccess: async () => {
      setError(null);
      setEditing(false);
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: maskedKey }),
        queryClient.invalidateQueries({ queryKey: revealKey }),
      ]);
    },
    onError: (e: Error) => setError(e.message),
  });

  async function startEdit() {
    setError(null);
    setPreparing(true);
    try {
      setDraft(serializeDotenv(await ensureRevealed()));
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
            Runtime variables, encrypted at rest. Changes apply on the next deploy.
          </p>
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
            for comments. Saving replaces the whole set.
          </p>
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          <div className="flex gap-2">
            <Button loading={save.isPending} type="submit">
              Save variables
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
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteDomain(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  const addPreview = useMutation({
    mutationFn: () => {
      const ip = selectedServer?.sshHost ?? "";
      const previewDomainHost = `${app.slug}-${app.id.slice(0, 8)}.${ip}.sslip.io`;
      return createDomain(serverId, { host: previewDomainHost, upstream, appId: app.id });
    },
    onSuccess: async () => {
      setError(null);
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

function DisabledDomainsSection() {
  return (
    <Card className="p-6 opacity-70">
      <h2 className="font-semibold text-lg">Domains</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Domain management is disabled while this app deploys to multiple servers. Select one target
        server, or add a load balancer/shared ingress before attaching domains here.
      </p>
    </Card>
  );
}
