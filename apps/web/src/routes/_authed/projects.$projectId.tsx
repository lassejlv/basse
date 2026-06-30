import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowLeftIcon, BoxIcon, ChevronRightIcon, PlusIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import type { App, AppBuildRunner, AppSourceType } from "@basse/shared";
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
import { createApp, listApps } from "@/lib/apps";
import { createEnvironment, listEnvironments } from "@/lib/environments";
import { relativeTime } from "@/lib/format";
import { getProject } from "@/lib/projects";
import { listServers } from "@/lib/servers";

export const Route = createFileRoute("/_authed/projects/$projectId")({
  component: ProjectDetailRoute,
});

function ProjectDetailRoute() {
  const { projectId } = Route.useParams();
  const [activeEnv, setActiveEnv] = useState<string | null>(null);

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
          {selectedEnv ? <CreateAppDialog environmentId={selectedEnv} /> : null}
        </div>
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
        <EmptyContent>
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
  const source = app.sourceType === "image" ? app.imageRef : app.repositoryUrl;
  return (
    <Link
      className="group flex items-center gap-4 px-4 py-3.5 transition hover:bg-accent/40"
      params={{ appId: app.id }}
      to="/apps/$appId"
    >
      <StatusDot status={app.latestDeploymentStatus} />
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium text-sm">{app.name}</p>
        <p className="truncate font-mono text-muted-foreground text-xs">{source}</p>
      </div>
      <div className="hidden shrink-0 items-center gap-2 sm:flex">
        <DeployStatusBadge size="sm" status={app.latestDeploymentStatus} />
        <Badge size="sm" variant="outline">
          :{app.port}
        </Badge>
      </div>
      <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground/50 transition group-hover:translate-x-0.5 group-hover:text-muted-foreground" />
    </Link>
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
  const [sourceType, setSourceType] = useState<AppSourceType>("repository");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [imageRef, setImageRef] = useState("");
  const [branch, setBranch] = useState("main");
  const [port, setPort] = useState("3000");
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [buildRunner, setBuildRunner] = useState<AppBuildRunner>("depot");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setSourceType("repository");
    setRepositoryUrl("");
    setImageRef("");
    setBranch("main");
    setPort("3000");
    setServerIds([]);
    setBuildRunner("depot");
    setError(null);
  }

  const localBuildInvalid = buildRunner === "server" && serverIds.length !== 1;
  const add = useMutation({
    mutationFn: () =>
      createApp({
        environmentId,
        name,
        sourceType,
        repositoryUrl,
        imageRef,
        branch,
        port: Number(port),
        serverIds,
        buildRunner,
      }),
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
    add.mutate();
  }

  function toggleServer(serverId: string, checked: boolean) {
    setServerIds((current) =>
      checked
        ? [...new Set([...current, serverId])]
        : current.filter((selectedServerId) => selectedServerId !== serverId),
    );
  }

  const serverList = servers.data ?? [];

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
            <DialogDescription>Deploy from a Git repository or a Docker image.</DialogDescription>
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
            {sourceType === "repository" ? (
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
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button
              disabled={!name.trim() || localBuildInvalid}
              loading={add.isPending}
              type="submit"
            >
              Create app
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
