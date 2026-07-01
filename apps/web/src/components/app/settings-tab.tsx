import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, useNavigate } from "@tanstack/react-router";
import { PlusIcon, TrashIcon } from "lucide-react";
import { useEffect, useState } from "react";
import type { AppBuildMode, AppBuildRunner, AppSourceType, AppVolume } from "@basse/shared";
import { DatabaseIcon, databaseEngineLabel } from "@/components/database-icon";
import { GitHubRepositorySelect } from "@/components/github-repository-select";
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
import { Switch } from "@/components/ui/switch";
import type { App } from "@/lib/apps";
import { deleteApp } from "@/lib/apps";
import { stageAppChanges } from "@/lib/changes";
import { formatBytes } from "@/lib/format";
import { listGitHubRepositories } from "@/lib/github";
import { getAgentInfo, listServers } from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";
import {
  databaseDefaultPort,
  formatCpuCores,
  formatCpuInput,
  parseCpuLimit,
  parseMemoryLimit,
  sliderToCpuInput,
  sliderToMemoryInput,
} from "./shared";

/** The full settings stack for an app: build/database, servers, resources,
 * volumes, delete. `app` is live state, `draft` has staged changes overlaid. */
export function AppSettingsTab({ app, draft }: { app: App; draft: App }) {
  return (
    <div className="flex flex-col gap-6">
      {app.appKind === "database" ? (
        <DatabaseSettingsCard app={draft} />
      ) : (
        <BuildSettingsCard app={draft} />
      )}
      <ServerCard app={draft} />
      <ResourceLimitsCard app={draft} />
      {app.appKind === "service" ? <VolumesCard app={draft} /> : null}
      <DeleteAppCard app={app} />
    </div>
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
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
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
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
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
  const githubRepositories = useQuery({
    queryKey: ["github-repositories", "build-settings"],
    queryFn: listGitHubRepositories,
  });
  const [sourceType, setSourceType] = useState<AppSourceType>(app.sourceType);
  const [repositoryUrl, setRepositoryUrl] = useState(app.repositoryUrl);
  const [imageRef, setImageRef] = useState(app.imageRef ?? "");
  const [branch, setBranch] = useState(app.branch);
  const [port, setPort] = useState(String(app.port));
  const [buildMode, setBuildMode] = useState<AppBuildMode>(app.buildMode);
  const [buildRootDirectory, setBuildRootDirectory] = useState(app.buildRootDirectory);
  const [dockerfilePath, setDockerfilePath] = useState(app.dockerfilePath);
  const [autoRedeployEnabled, setAutoRedeployEnabled] = useState(app.autoRedeployEnabled);
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
    setBuildMode(app.buildMode);
    setBuildRootDirectory(app.buildRootDirectory);
    setDockerfilePath(app.dockerfilePath);
    setAutoRedeployEnabled(app.autoRedeployEnabled);
  }, [
    app.sourceType,
    app.repositoryUrl,
    app.imageRef,
    app.branch,
    app.port,
    app.buildMode,
    app.buildRootDirectory,
    app.dockerfilePath,
    app.autoRedeployEnabled,
  ]);

  const update = useMutation({
    mutationFn: (input: {
      sourceType?: AppSourceType;
      repositoryUrl?: string;
      imageRef?: string | null;
      branch?: string;
      port?: number;
      buildMode?: AppBuildMode;
      buildRootDirectory?: string;
      dockerfilePath?: string;
      buildRunner?: AppBuildRunner;
      autoRedeployEnabled?: boolean;
    }) => stageAppChanges(app.id, input),
    onSuccess: (data) => {
      setError(null);
      toast.success("Build change staged");
      queryClient.setQueryData(["changes", app.id], data);
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
    },
    onError: (e: Error) => setError(e.message),
  });
  const localBuildInvalid = app.buildRunner === "server" && app.serverIds.length !== 1;
  const githubRepoList = githubRepositories.data?.repositories ?? [];
  const githubRepoErrors = githubRepositories.data?.errors ?? [];

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
          update.mutate(
            sourceType === "image"
              ? {
                  sourceType,
                  imageRef,
                  port: Number(port),
                }
              : {
                  sourceType,
                  repositoryUrl,
                  imageRef: null,
                  branch,
                  port: Number(port),
                  buildMode,
                  buildRootDirectory,
                  dockerfilePath,
                  autoRedeployEnabled,
                },
          );
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
            {githubRepoList.length > 0 ? (
              <GitHubRepositorySelect
                label="Private GitHub repository"
                onSelect={(repository) => {
                  setRepositoryUrl(repository.cloneUrl);
                  setBranch(repository.defaultBranch);
                }}
                repositories={githubRepoList}
                value={repositoryUrl}
              />
            ) : null}
            {githubRepositories.isError ? (
              <p className="text-destructive-foreground text-sm">
                Couldn't load installed GitHub repositories: {toMessage(githubRepositories.error)}
              </p>
            ) : githubRepoErrors.length > 0 ? (
              <p className="text-muted-foreground text-sm">
                Some GitHub installations could not be loaded: {githubRepoErrors.join("; ")}
              </p>
            ) : !githubRepositories.isPending && githubRepoList.length === 0 ? (
              <p className="text-muted-foreground text-sm">
                Need a private repository?{" "}
                <Link
                  className="underline underline-offset-4"
                  search={{
                    code: undefined,
                    installation_id: undefined,
                    setup_action: undefined,
                    state: undefined,
                  }}
                  to="/secrets"
                >
                  Install the GitHub App in Secrets
                </Link>
                .
              </p>
            ) : null}
            <div className="space-y-2">
              <Label htmlFor="app-source-repo">Public or manual repository URL</Label>
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
            <div className="space-y-2">
              <Label>Build mode</Label>
              <Select
                value={buildMode}
                onValueChange={(value) => setBuildMode((value ?? "auto") as AppBuildMode)}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Build mode">
                    {(value: AppBuildMode) =>
                      value === "dockerfile"
                        ? "Force Dockerfile"
                        : value === "railpack"
                          ? "Force Railpack"
                          : "Auto detect"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  <SelectItem value="auto">Auto detect</SelectItem>
                  <SelectItem value="dockerfile">Force Dockerfile</SelectItem>
                  <SelectItem value="railpack">Force Railpack</SelectItem>
                </SelectPopup>
              </Select>
            </div>
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="space-y-2">
                <Label htmlFor="app-build-root">Root directory</Label>
                <Input
                  id="app-build-root"
                  onChange={(event) => setBuildRootDirectory(event.currentTarget.value)}
                  placeholder="."
                  value={buildRootDirectory}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="app-dockerfile-path">Dockerfile path</Label>
                <Input
                  id="app-dockerfile-path"
                  onChange={(event) => setDockerfilePath(event.currentTarget.value)}
                  placeholder="Dockerfile"
                  value={dockerfilePath}
                />
              </div>
            </div>
            <label className="flex items-center justify-between gap-4 rounded-md border px-3 py-2">
              <span className="min-w-0">
                <span className="block font-medium text-sm">Auto redeploy</span>
                <span className="block text-muted-foreground text-xs">
                  Deploy this app automatically when GitHub receives a push on this branch.
                </span>
              </span>
              <Switch checked={autoRedeployEnabled} onCheckedChange={setAutoRedeployEnabled} />
            </label>
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
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
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
                {cpuCores.trim()
                  ? formatCpuCores(Math.round(Number(cpuCores) * 1000))
                  : "Unlimited"}
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
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
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
