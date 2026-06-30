import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  BoxIcon,
  ChevronRightIcon,
  DownloadIcon,
  PlusIcon,
  RefreshCwIcon,
  TrashIcon,
} from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import type {
  App,
  AppBuildRunner,
  AppKind,
  AppSourceType,
  DatabaseKind,
  ImportableDockerContainer,
} from "@basse/shared";
import { DatabaseIcon, databaseEngineLabel } from "@/components/database-icon";
import { DeployStatusBadge, StatusDot } from "@/components/deploy-status";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Checkbox } from "@/components/ui/checkbox";
import {
  Dialog,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Tabs, TabsList, TabsTab } from "@/components/ui/tabs";
import {
  createApp,
  importDockerContainer,
  listApps,
  listImportableDockerContainers,
} from "@/lib/apps";
import { triggerDeploy } from "@/lib/deployments";
import { createEnvironment, listEnvironments } from "@/lib/environments";
import { relativeTime } from "@/lib/format";
import { deleteProject, getProject } from "@/lib/projects";
import { listServers } from "@/lib/servers";

export const Route = createFileRoute("/_authed/projects/$projectId")({
  component: ProjectDetailRoute,
});

function ProjectDetailRoute() {
  const { projectId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const [activeEnv, setActiveEnv] = useState<string | null>(null);
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const project = useQuery({
    queryKey: ["project", projectId],
    queryFn: () => getProject(projectId),
  });
  const environments = useQuery({
    queryKey: ["environments", projectId],
    queryFn: () => listEnvironments(projectId),
  });

  const envList = environments.data ?? [];
  const selectedEnv = activeEnv ?? envList[0]?.id ?? null;

  const removeProject = useMutation({
    mutationFn: () => deleteProject(projectId),
    onSuccess: async () => {
      setDeleteError(null);
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      await navigate({ to: "/projects" });
    },
    onError: (mutationError: Error) => setDeleteError(mutationError.message),
  });

  function confirmDeleteProject() {
    const name = project.data?.name ?? "this project";
    if (
      !window.confirm(
        `Delete ${name}? This removes its environments, apps, and running app containers.`,
      )
    ) {
      return;
    }
    removeProject.mutate();
  }

  if (project.isPending) {
    return <p className="p-4 text-muted-foreground text-sm md:p-6">Loading…</p>;
  }
  if (project.isError || !project.data) {
    return <p className="p-4 text-destructive-foreground text-sm md:p-6">Project not found.</p>;
  }

  return (
    <section className="flex flex-1 flex-col gap-7 p-4 md:p-6">
      <div>
        <Link
          className="inline-flex items-center gap-1.5 text-muted-foreground text-sm transition hover:text-foreground"
          to="/projects"
        >
          <ArrowLeftIcon className="size-4" />
          Projects
        </Link>
        <div className="mt-3 flex flex-wrap items-end justify-between gap-4">
          <div>
            <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
              Project
            </p>
            <h1 className="mt-1 font-semibold text-2xl tracking-tight md:text-3xl">
              {project.data.name}
            </h1>
            <p className="mt-1 font-mono text-muted-foreground text-xs">
              {project.data.slug} · created {relativeTime(project.data.createdAt)}
            </p>
          </div>
          <div className="flex items-center gap-2">
            <Button
              loading={removeProject.isPending}
              onClick={confirmDeleteProject}
              variant="destructive-outline"
            >
              <TrashIcon />
              Delete
            </Button>
            {selectedEnv ? (
              <>
                <ImportContainerDialog environmentId={selectedEnv} />
                <CreateAppDialog environmentId={selectedEnv} />
              </>
            ) : null}
          </div>
        </div>
        {deleteError ? (
          <p className="mt-3 text-destructive-foreground text-sm">{deleteError}</p>
        ) : null}
      </div>

      <div className="flex flex-wrap items-center gap-2">
        {environments.isPending ? (
          <div className="h-9 w-40 animate-pulse rounded-lg bg-muted/50" aria-hidden />
        ) : (
          <Tabs onValueChange={(value) => setActiveEnv(value as string)} value={selectedEnv ?? ""}>
            <TabsList>
              {envList.map((env) => (
                <TabsTab key={env.id} value={env.id}>
                  {env.name}
                </TabsTab>
              ))}
            </TabsList>
          </Tabs>
        )}
        <NewEnvironmentDialog projectId={projectId} />
      </div>

      {selectedEnv ? <EnvironmentApps environmentId={selectedEnv} /> : null}
    </section>
  );
}

function EnvironmentApps({ environmentId }: { environmentId: string }) {
  const apps = useQuery({
    queryKey: ["apps", environmentId],
    queryFn: () => listApps(environmentId),
  });
  const appList = apps.data ?? [];

  if (apps.isPending) {
    return <div className="h-40 animate-pulse rounded-2xl border bg-muted/30" aria-hidden />;
  }

  if (appList.length === 0) {
    return (
      <Empty className="rounded-2xl border border-dashed">
        <EmptyHeader>
          <EmptyMedia variant="icon">
            <BoxIcon />
          </EmptyMedia>
          <EmptyTitle>No apps in this environment</EmptyTitle>
          <EmptyDescription>
            Deploy from a Git repository or a prebuilt Docker image.
          </EmptyDescription>
        </EmptyHeader>
        <EmptyContent className="flex flex-wrap items-center justify-center gap-2">
          <ImportContainerDialog environmentId={environmentId} />
          <CreateAppDialog environmentId={environmentId} />
        </EmptyContent>
      </Empty>
    );
  }

  return (
    <Card className="divide-y overflow-hidden p-0">
      {appList.map((app) => (
        <AppRow key={app.id} app={app} />
      ))}
    </Card>
  );
}

function AppRow({ app }: { app: App }) {
  const database = app.appKind === "database" ? app.database : null;
  const source = database
    ? `${databaseEngineLabel(database.kind)} ${database.version ?? ""}`
    : app.sourceType === "image"
      ? app.imageRef
      : app.repositoryUrl;
  return (
    <Link
      className="group flex items-center gap-4 px-4 py-3.5 transition hover:bg-accent/40"
      params={{ appId: app.id }}
      to="/apps/$appId"
    >
      <StatusDot status={app.latestDeploymentStatus} />
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium text-sm">{app.name}</p>
        <p className="truncate font-mono text-muted-foreground text-xs">
          {database ? (
            <span className="inline-flex max-w-full items-center gap-1.5 align-middle">
              <DatabaseIcon className="size-3.5" kind={database.kind} />
              <span className="truncate">{source}</span>
            </span>
          ) : (
            source
          )}
        </p>
      </div>
      <div className="hidden shrink-0 items-center gap-2 sm:flex">
        {app.appKind === "database" ? (
          <Badge size="sm" variant="outline">
            database
          </Badge>
        ) : null}
        <DeployStatusBadge size="sm" status={app.latestDeploymentStatus} />
        <Badge size="sm" variant="outline">
          :{app.port}
        </Badge>
      </div>
      <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground/50 transition group-hover:translate-x-0.5 group-hover:text-muted-foreground" />
    </Link>
  );
}

function firstContainerPort(container: ImportableDockerContainer): number {
  return container.ports.find((port) => port.privatePort > 0)?.privatePort ?? 3000;
}

function containerPortsLabel(container: ImportableDockerContainer): string {
  const ports = container.ports
    .filter((port) => port.privatePort > 0)
    .map((port) =>
      port.publicPort
        ? `${port.publicPort}:${port.privatePort}/${port.type}`
        : `${port.privatePort}/${port.type}`,
    );
  return ports.length > 0 ? ports.join(", ") : "No exposed ports";
}

function ImportContainerDialog({ environmentId }: { environmentId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [serverId, setServerId] = useState("");
  const [containerId, setContainerId] = useState("");
  const [name, setName] = useState("");
  const [port, setPort] = useState("3000");
  const [error, setError] = useState<string | null>(null);

  const servers = useQuery({ queryKey: ["servers", "for-container-import"], queryFn: listServers });
  const activeServers = (servers.data ?? []).filter((server) => server.status === "active");

  useEffect(() => {
    if (open && !serverId && activeServers[0]) {
      setServerId(activeServers[0].id);
    }
  }, [activeServers, open, serverId]);

  const containers = useQuery({
    queryKey: ["importable-containers", serverId],
    queryFn: () => listImportableDockerContainers(serverId),
    enabled: open && Boolean(serverId),
  });
  const containerList = containers.data ?? [];
  const selectedContainer = containerList.find((container) => container.id === containerId);

  function reset() {
    setContainerId("");
    setName("");
    setPort("3000");
    setError(null);
  }

  function selectContainer(container: ImportableDockerContainer) {
    if (!container.running) return;
    setContainerId(container.id);
    setName(container.name);
    setPort(String(firstContainerPort(container)));
  }

  const importMutation = useMutation({
    mutationFn: () =>
      importDockerContainer({
        environmentId,
        serverId,
        containerId,
        name,
        port: Number(port),
      }),
    onSuccess: async () => {
      reset();
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["apps", environmentId] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!serverId) {
      setError("Choose an active server.");
      return;
    }
    if (!containerId) {
      setError("Choose a running container.");
      return;
    }
    if (!name.trim()) {
      setError("Name is required.");
      return;
    }
    const numericPort = Number(port);
    if (!Number.isInteger(numericPort) || numericPort < 1 || numericPort > 65535) {
      setError("Port must be a valid port.");
      return;
    }
    importMutation.mutate();
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) reset();
      }}
    >
      <DialogTrigger
        render={
          <Button variant="outline">
            <DownloadIcon />
            Import
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-2xl">
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Import container</DialogTitle>
            <DialogDescription>
              Take over a running Docker container that is not already tracked by Basse.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
              <div className="space-y-2">
                <Label>Server</Label>
                <Select
                  value={serverId}
                  onValueChange={(value) => {
                    setServerId(value ?? "");
                    setContainerId("");
                  }}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="Server">
                      {(value: string) =>
                        activeServers.find((server) => server.id === value)?.name ?? "Server"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    {activeServers.map((server) => (
                      <SelectItem key={server.id} value={server.id}>
                        {server.name}
                      </SelectItem>
                    ))}
                  </SelectPopup>
                </Select>
              </div>
              <div className="flex items-end">
                <Button
                  disabled={!serverId || containers.isFetching}
                  onClick={() => void containers.refetch()}
                  type="button"
                  variant="outline"
                >
                  <RefreshCwIcon />
                  Scan
                </Button>
              </div>
            </div>
            {servers.isPending ? (
              <p className="text-muted-foreground text-sm">Loading servers…</p>
            ) : activeServers.length === 0 ? (
              <p className="text-muted-foreground text-sm">
                No active servers are ready for container import.
              </p>
            ) : containers.isPending && serverId ? (
              <div className="h-28 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
            ) : containerList.length === 0 && serverId ? (
              <p className="rounded-lg border border-dashed px-3 py-6 text-center text-muted-foreground text-sm">
                No unmanaged containers found on this server.
              </p>
            ) : (
              <div className="max-h-72 overflow-y-auto rounded-lg border">
                {containerList.map((container) => {
                  const selected = container.id === containerId;
                  return (
                    <button
                      className="flex w-full min-w-0 items-center justify-between gap-3 border-b px-3 py-2.5 text-left text-sm last:border-b-0 hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-60 data-[selected=true]:bg-accent"
                      data-selected={selected}
                      disabled={!container.running}
                      key={container.id}
                      onClick={() => selectContainer(container)}
                      type="button"
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{container.name}</span>
                        <span className="block truncate font-mono text-muted-foreground text-xs">
                          {container.image}
                        </span>
                      </span>
                      <span className="shrink-0 text-right">
                        <Badge size="sm" variant={container.running ? "outline" : "secondary"}>
                          {container.running ? "running" : container.state}
                        </Badge>
                        <span className="mt-1 block font-mono text-muted-foreground text-xs">
                          {containerPortsLabel(container)}
                        </span>
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
            {selectedContainer ? (
              <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                <div className="space-y-2">
                  <Label htmlFor="import-app-name">App name</Label>
                  <Input
                    id="import-app-name"
                    onChange={(event) => setName(event.currentTarget.value)}
                    value={name}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="import-app-port">Port</Label>
                  <Input
                    id="import-app-port"
                    onChange={(event) => setPort(event.currentTarget.value)}
                    type="number"
                    value={port}
                  />
                </div>
              </div>
            ) : null}
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button
              disabled={!containerId || !name.trim() || !serverId}
              loading={importMutation.isPending}
              type="submit"
            >
              Import container
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

function NewEnvironmentDialog({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () => createEnvironment(projectId, name),
    onSuccess: async () => {
      setName("");
      setError(null);
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["environments", projectId] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setError(null);
      }}
    >
      <DialogTrigger
        render={
          <Button size="sm" variant="outline">
            <PlusIcon />
            Environment
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-md">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            add.mutate();
          }}
        >
          <DialogHeader>
            <DialogTitle>New environment</DialogTitle>
            <DialogDescription>
              Environments isolate apps and variables — for example staging or preview.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-2">
            <Label htmlFor="environment-name">Name</Label>
            <Input
              autoFocus
              id="environment-name"
              onChange={(event) => setName(event.currentTarget.value)}
              placeholder="staging"
              required
              value={name}
            />
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button disabled={!name.trim()} loading={add.isPending} type="submit">
              Add environment
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

function CreateAppDialog({ environmentId }: { environmentId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const servers = useQuery({ queryKey: ["servers", "for-apps"], queryFn: listServers });

  const [name, setName] = useState("");
  const [appKind, setAppKind] = useState<AppKind>("service");
  const [databaseKind, setDatabaseKind] = useState<DatabaseKind>("postgres");
  const [sourceType, setSourceType] = useState<AppSourceType>("repository");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [imageRef, setImageRef] = useState("");
  const [branch, setBranch] = useState("main");
  const [port, setPort] = useState("3000");
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [buildRunner, setBuildRunner] = useState<AppBuildRunner>("depot");
  const [databaseVersion, setDatabaseVersion] = useState("18");
  const [databaseName, setDatabaseName] = useState("");
  const [databaseUser, setDatabaseUser] = useState("postgres");
  const [databasePassword, setDatabasePassword] = useState("");
  const [databasePublicEnabled, setDatabasePublicEnabled] = useState(false);
  const [databasePublicPort, setDatabasePublicPort] = useState("5432");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setAppKind("service");
    setDatabaseKind("postgres");
    setSourceType("repository");
    setRepositoryUrl("");
    setImageRef("");
    setBranch("main");
    setPort("3000");
    setServerIds([]);
    setBuildRunner("depot");
    setDatabaseVersion("18");
    setDatabaseName("");
    setDatabaseUser("postgres");
    setDatabasePassword("");
    setDatabasePublicEnabled(false);
    setDatabasePublicPort("5432");
    setError(null);
  }

  const localBuildInvalid =
    appKind === "service" && buildRunner === "server" && serverIds.length !== 1;
  const databaseServerInvalid = appKind === "database" && serverIds.length !== 1;
  const add = useMutation({
    mutationFn: async () => {
      if (appKind === "database") {
        const created = await createApp({
          environmentId,
          name,
          appKind: "database",
          serverIds,
          databaseKind,
          databaseVersion,
          databaseName: databaseKind === "postgres" ? databaseName : undefined,
          databaseUser: databaseKind === "postgres" ? databaseUser : undefined,
          databasePassword: databasePassword || undefined,
          databasePublicEnabled,
          databasePublicPort: databasePublicEnabled ? Number(databasePublicPort) : null,
        });
        await triggerDeploy(created.id);
        return created;
      }

      return createApp({
        environmentId,
        name,
        sourceType,
        repositoryUrl,
        imageRef,
        branch,
        port: Number(port),
        serverIds,
        buildRunner,
      });
    },
    onSuccess: async () => {
      reset();
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["apps", environmentId] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (localBuildInvalid) {
      setError("Selected-server builds require exactly one server.");
      return;
    }
    if (databaseServerInvalid) {
      setError("Databases require exactly one server.");
      return;
    }
    add.mutate();
  }

  function toggleServer(serverId: string, checked: boolean) {
    setServerIds((current) =>
      appKind === "database"
        ? checked
          ? [serverId]
          : []
        : checked
          ? [...new Set([...current, serverId])]
          : current.filter((selectedServerId) => selectedServerId !== serverId),
    );
  }

  const serverList = servers.data ?? [];

  function updateDatabaseKind(kind: DatabaseKind) {
    setDatabaseKind(kind);
    if (kind === "postgres") {
      setDatabaseVersion("18");
      setDatabaseUser("postgres");
      setDatabasePublicPort("5432");
    } else {
      setDatabaseVersion("8");
      setDatabaseName("");
      setDatabaseUser("");
      setDatabasePublicPort("6379");
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setError(null);
      }}
    >
      <DialogTrigger
        render={
          <Button>
            <PlusIcon />
            New app
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-lg">
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New app</DialogTitle>
            <DialogDescription>
              Deploy an application or create a managed database.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="app-name">Name</Label>
              <Input
                autoFocus
                id="app-name"
                onChange={(event) => setName(event.currentTarget.value)}
                placeholder="web"
                required
                value={name}
              />
            </div>
            <div className="space-y-2">
              <Label>Type</Label>
              <Select
                value={appKind}
                onValueChange={(value) => {
                  setAppKind((value ?? "service") as AppKind);
                  setServerIds([]);
                }}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Type">
                    {(value: AppKind) =>
                      value === "database" ? "Managed database" : "Application"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  <SelectItem value="service">Application</SelectItem>
                  <SelectItem value="database">Managed database</SelectItem>
                </SelectPopup>
              </Select>
            </div>
            {appKind === "service" ? (
              <>
                <div className="space-y-2">
                  <Label>Source</Label>
                  <Select
                    value={sourceType}
                    onValueChange={(value) =>
                      setSourceType((value ?? "repository") as AppSourceType)
                    }
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
                      <Label htmlFor="app-repo">Repository URL</Label>
                      <Input
                        id="app-repo"
                        onChange={(event) => setRepositoryUrl(event.currentTarget.value)}
                        placeholder="https://github.com/user/repo"
                        required
                        value={repositoryUrl}
                      />
                    </div>
                    <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                      <div className="space-y-2">
                        <Label htmlFor="app-branch">Branch</Label>
                        <Input
                          id="app-branch"
                          onChange={(event) => setBranch(event.currentTarget.value)}
                          value={branch}
                        />
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="app-port">Port</Label>
                        <Input
                          id="app-port"
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
                      <Label htmlFor="app-image">Docker image</Label>
                      <Input
                        id="app-image"
                        onChange={(event) => setImageRef(event.currentTarget.value)}
                        placeholder="nginx:alpine"
                        required
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
              </>
            ) : (
              <>
                <div className="space-y-2">
                  <Label>Engine</Label>
                  <Select
                    value={databaseKind}
                    onValueChange={(value) =>
                      updateDatabaseKind((value ?? "postgres") as DatabaseKind)
                    }
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Engine">
                        {(value: DatabaseKind) => (
                          <span className="flex items-center gap-2">
                            <DatabaseIcon className="size-4" kind={value} />
                            <span>{databaseEngineLabel(value)}</span>
                          </span>
                        )}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      <SelectItem value="postgres">
                        <span className="flex items-center gap-2">
                          <DatabaseIcon className="size-4" kind="postgres" />
                          <span>Postgres</span>
                        </span>
                      </SelectItem>
                      <SelectItem value="redis">
                        <span className="flex items-center gap-2">
                          <DatabaseIcon className="size-4" kind="redis" />
                          <span>Redis</span>
                        </span>
                      </SelectItem>
                    </SelectPopup>
                  </Select>
                </div>
                <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                  {databaseKind === "postgres" ? (
                    <div className="space-y-2">
                      <Label htmlFor="database-name">Database name</Label>
                      <Input
                        id="database-name"
                        onChange={(event) => setDatabaseName(event.currentTarget.value)}
                        placeholder={name || "app"}
                        value={databaseName}
                      />
                    </div>
                  ) : null}
                  <div className="space-y-2">
                    <Label htmlFor="database-version">Version</Label>
                    <Input
                      id="database-version"
                      onChange={(event) => setDatabaseVersion(event.currentTarget.value)}
                      value={databaseVersion}
                    />
                  </div>
                </div>
                <div className="grid gap-3 sm:grid-cols-2">
                  {databaseKind === "postgres" ? (
                    <div className="space-y-2">
                      <Label htmlFor="database-user">User</Label>
                      <Input
                        id="database-user"
                        onChange={(event) => setDatabaseUser(event.currentTarget.value)}
                        value={databaseUser}
                      />
                    </div>
                  ) : null}
                  <div className="space-y-2">
                    <Label htmlFor="database-password">Password</Label>
                    <Input
                      id="database-password"
                      onChange={(event) => setDatabasePassword(event.currentTarget.value)}
                      placeholder="Generate"
                      type="password"
                      value={databasePassword}
                    />
                  </div>
                </div>
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={databasePublicEnabled}
                    onCheckedChange={(value) => setDatabasePublicEnabled(value === true)}
                  />
                  <span>Enable public TCP access</span>
                </label>
                {databasePublicEnabled ? (
                  <div className="max-w-40 space-y-2">
                    <Label htmlFor="database-public-port">Public port</Label>
                    <Input
                      id="database-public-port"
                      onChange={(event) => setDatabasePublicPort(event.currentTarget.value)}
                      type="number"
                      value={databasePublicPort}
                    />
                  </div>
                ) : null}
              </>
            )}
            <div className="space-y-2">
              <Label>Servers</Label>
              {servers.isPending ? (
                <p className="text-muted-foreground text-sm">Loading servers…</p>
              ) : serverList.length === 0 ? (
                <p className="text-muted-foreground text-sm">
                  No servers yet —{" "}
                  <Link className="font-medium text-foreground underline" to="/servers">
                    add one
                  </Link>{" "}
                  first (you can also attach it later).
                </p>
              ) : (
                <div className="flex flex-col gap-2">
                  {serverList.map((server) => (
                    <label
                      key={server.id}
                      className="flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-sm"
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{server.name}</span>
                        <span className="text-muted-foreground">{server.status}</span>
                      </span>
                      <Checkbox
                        checked={serverIds.includes(server.id)}
                        onCheckedChange={(value) => toggleServer(server.id, value === true)}
                      />
                    </label>
                  ))}
                </div>
              )}
            </div>
            {appKind === "service" && sourceType === "repository" ? (
              <div className="space-y-2">
                <Label>Build location</Label>
                <Select
                  value={buildRunner}
                  onValueChange={(value) => setBuildRunner((value ?? "depot") as AppBuildRunner)}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="Build location">
                      {(value: AppBuildRunner) =>
                        value === "server" ? "Selected server" : "Depot"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    <SelectItem value="depot">Depot</SelectItem>
                    <SelectItem value="server">Selected server</SelectItem>
                  </SelectPopup>
                </Select>
                {localBuildInvalid ? (
                  <p className="text-warning-foreground text-sm">
                    Selected-server builds require exactly one server. Use Depot for multiple
                    servers.
                  </p>
                ) : null}
              </div>
            ) : null}
            {databaseServerInvalid ? (
              <p className="text-warning-foreground text-sm">
                Databases require exactly one server.
              </p>
            ) : null}
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button
              disabled={!name.trim() || localBuildInvalid || databaseServerInvalid}
              loading={add.isPending}
              type="submit"
            >
              {appKind === "database" ? "Create database" : "Create app"}
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
