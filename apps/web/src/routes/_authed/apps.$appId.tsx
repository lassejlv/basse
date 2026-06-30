import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { FitAddon } from "@xterm/addon-fit";
import { Terminal } from "@xterm/xterm";
import "@xterm/xterm/css/xterm.css";
import { PlusIcon, TrashIcon } from "lucide-react";
import { FormEvent, useEffect, useRef, useState } from "react";
import type { AppBuildRunner, AppSourceType, AppVolume, DeploymentStatus } from "@basse/shared";
import { chartCssVars } from "@/components/charts/chart-context";
import { Grid } from "@/components/charts/grid";
import { Line, LineChart } from "@/components/charts/line-chart";
import { ChartTooltip } from "@/components/charts/tooltip";
import { XAxis } from "@/components/charts/x-axis";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { Textarea } from "@/components/ui/textarea";
import type { App } from "@/lib/apps";
import {
  getApp,
  getAppLogs,
  getAppMetrics,
  runAppConsoleCommand,
  stopAppContainer,
  updateApp,
} from "@/lib/apps";
import { listDeployments, triggerDeploy } from "@/lib/deployments";
import { createDomain, deleteDomain, listDomains } from "@/lib/domains";
import { listEnvVars, setEnvVars } from "@/lib/env-vars";
import { listServers } from "@/lib/servers";

export const Route = createFileRoute("/_authed/apps/$appId")({
  component: AppDetailRoute,
});

const DEPLOY_STATUS_VARIANT: Record<
  DeploymentStatus,
  "outline" | "info" | "success" | "error" | "secondary"
> = {
  queued: "outline",
  building: "info",
  deploying: "info",
  healthy: "success",
  superseded: "secondary",
  failed: "error",
  cancelled: "secondary",
};

function AppDetailRoute() {
  const { appId } = Route.useParams();
  const app = useQuery({ queryKey: ["app", appId], queryFn: () => getApp(appId) });

  if (app.isPending) {
    return <p className="p-6 text-muted-foreground text-sm">Loading…</p>;
  }
  if (app.isError || !app.data) {
    return <p className="p-6 text-destructive-foreground text-sm">App not found.</p>;
  }

  const data = app.data;
  const canDeploy =
    data.serverIds.length > 0 &&
    (data.sourceType === "image" || data.buildRunner !== "server" || data.serverIds.length === 1);

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">{data.name}</h1>
        <p className="mt-2 font-mono text-muted-foreground text-sm">
          {data.sourceType === "image" ? data.imageRef : data.repositoryUrl} · :{data.port} ·{" "}
          {data.sourceType === "repository" ? `${data.branch} · ${data.buildMode} · ${data.buildRunner}` : "image"}
        </p>
        {data.serverIds.length === 0 ? (
          <p className="mt-2 text-warning-foreground text-sm">
            No servers attached — select at least one before deploying.
          </p>
        ) : null}
      </div>

      <ServerCard app={data} />
      <BuildSettingsCard app={data} />
      <DeploySection appId={appId} canDeploy={canDeploy} />
      <RuntimeCard app={data} />
      <EnvVarsCard appId={appId} />
      <VolumesCard app={data} />
      {data.serverIds.length === 1 ? (
        <AppDomainsSection app={data} serverId={data.serverIds[0]!} />
      ) : data.serverIds.length > 1 ? (
        <DisabledDomainsSection />
      ) : null}
    </section>
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
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Servers</h2>
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
    </div>
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
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Build</h2>
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
    </div>
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
    setVolumes((current) => [
      ...current,
      { hostPath: "", containerPath: "", readOnly: false },
    ]);
  }

  function removeVolume(index: number) {
    setVolumes((current) => current.filter((_, currentIndex) => currentIndex !== index));
  }

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Volumes</h2>
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
                      onChange={(event) => updateVolume(index, { hostPath: event.currentTarget.value })}
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
    </div>
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
      ]);
    },
    onError: (error: Error) => setStopError(error.message),
  });

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <div className="flex items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Runtime</h2>
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
        <p className="mt-5 text-muted-foreground text-sm">Select a server before using runtime tools.</p>
      ) : (
        <>
          <div className="mt-5 h-64 rounded-md border bg-muted/20 p-3">
            {samples.length > 1 ? (
              <LineChart
                animationDuration={700}
                aspectRatio={undefined}
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
            <span>CPU</span>
            <span>Memory</span>
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
    </div>
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

function DeploySection({ appId, canDeploy }: { appId: string; canDeploy: boolean }) {
  const queryClient = useQueryClient();
  const queryKey = ["deployments", appId];
  const [error, setError] = useState<string | null>(null);

  const deployments = useQuery({
    queryKey,
    queryFn: () => listDeployments(appId),
    // Poll while the newest deployment is still in flight.
    refetchInterval: (query) => {
      const latest = query.state.data?.[0];
      return latest && ["queued", "building", "deploying"].includes(latest.status) ? 2000 : false;
    },
  });

  const deploy = useMutation({
    mutationFn: () => triggerDeploy(appId),
    onSuccess: async () => {
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  const list = deployments.data ?? [];
  const latest = list[0];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">Deployments</h2>
        <Button disabled={!canDeploy} loading={deploy.isPending} onClick={() => deploy.mutate()}>
          Deploy
        </Button>
      </div>
      {error ? <p className="mt-2 text-destructive-foreground text-sm">{error}</p> : null}

      {latest ? (
        <pre className="mt-4 max-h-72 overflow-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
          {latest.logs ?? "Waiting for logs…"}
        </pre>
      ) : null}

      <div className="mt-4">
        {list.length === 0 ? (
          <p className="text-muted-foreground text-sm">No deployments yet.</p>
        ) : (
          <ul className="flex flex-col gap-1.5">
            {list.map((d) => (
              <li key={d.id} className="flex items-center justify-between gap-3 text-sm">
                <span className="font-mono text-muted-foreground text-xs">
                  {new Date(d.createdAt).toLocaleString()}
                  {d.commitSha ? ` · ${d.commitSha.slice(0, 7)}` : ""}
                </span>
                <Badge variant={DEPLOY_STATUS_VARIANT[d.status]}>{d.status}</Badge>
              </li>
            ))}
          </ul>
        )}
      </div>
    </div>
  );
}

function EnvVarsCard({ appId }: { appId: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["env-vars", appId];
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  const vars = useQuery({ queryKey, queryFn: () => listEnvVars(appId) });

  const save = useMutation({
    mutationFn: () => {
      const parsed = draft
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith("#"))
        .map((line) => {
          const eq = line.indexOf("=");
          return { key: line.slice(0, eq).trim(), value: line.slice(eq + 1) };
        })
        .filter((v) => v.key);
      return setEnvVars(appId, parsed);
    },
    onSuccess: async () => {
      setDraft("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  const list = vars.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Environment variables</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Runtime variables, encrypted at rest. Changes apply on the next deploy.
      </p>

      <div className="mt-5">
        {vars.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : list.length === 0 ? (
          <p className="text-muted-foreground text-sm">No variables set.</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {list.map((v) => (
              <li key={v.key} className="flex justify-between gap-3 font-mono text-sm">
                <span className="font-medium">{v.key}</span>
                <span className="text-muted-foreground">{v.valueHint}</span>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="mt-6 space-y-3 border-t pt-6">
        <p className="text-sm">
          Paste <code className="font-mono">KEY=value</code> lines to <strong>replace</strong> the
          whole set:
        </p>
        <Textarea
          className="font-mono text-xs"
          onChange={(e) => setDraft(e.currentTarget.value)}
          placeholder={"DATABASE_URL=postgres://…\nPORT=3000"}
          rows={5}
          value={draft}
        />
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button disabled={!draft.trim()} loading={save.isPending} onClick={() => save.mutate()}>
          Save variables
        </Button>
      </div>
    </div>
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
      const host = `${app.slug}-${app.id.slice(0, 8)}.${ip}.sslip.io`;
      return createDomain(serverId, { host, upstream, appId: app.id });
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
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Domains</h2>
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
                <span className="truncate font-medium text-sm">{d.host}</span>
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
    </div>
  );
}

function DisabledDomainsSection() {
  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6 opacity-70">
      <h2 className="text-lg font-semibold">Domains</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Domain management is disabled while this app deploys to multiple servers. Select one target
        server, or add a load balancer/shared ingress before attaching domains here.
      </p>
    </div>
  );
}
