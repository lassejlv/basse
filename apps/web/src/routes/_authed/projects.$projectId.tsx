import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowLeftIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectButton,
  SelectItem,
  SelectPopup,
  SelectValue,
} from "@/components/ui/select";
import { createApp, listApps } from "@/lib/apps";
import { createEnvironment, listEnvironments } from "@/lib/environments";
import { getProject } from "@/lib/projects";
import { listServers } from "@/lib/servers";

export const Route = createFileRoute("/_authed/projects/$projectId")({
  component: ProjectDetailRoute,
});

function ProjectDetailRoute() {
  const { projectId } = Route.useParams();
  const [activeEnv, setActiveEnv] = useState<string | null>(null);

  const project = useQuery({ queryKey: ["project", projectId], queryFn: () => getProject(projectId) });
  const environments = useQuery({
    queryKey: ["environments", projectId],
    queryFn: () => listEnvironments(projectId),
  });

  const envList = environments.data ?? [];
  const selectedEnv = activeEnv ?? envList[0]?.id ?? null;

  if (project.isPending) {
    return <p className="p-6 text-muted-foreground text-sm">Loading…</p>;
  }
  if (project.isError || !project.data) {
    return <p className="p-6 text-destructive-foreground text-sm">Project not found.</p>;
  }

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-3xl">
        <Link
          to="/projects"
          className="inline-flex items-center gap-1.5 text-muted-foreground text-sm hover:text-foreground"
        >
          <ArrowLeftIcon className="size-4" />
          Projects
        </Link>
        <h1 className="mt-3 text-2xl font-semibold tracking-normal md:text-3xl">
          {project.data.name}
        </h1>
      </div>

      <div className="max-w-3xl">
        <div className="flex flex-wrap items-center gap-2">
          {envList.map((env) => (
            <Button
              key={env.id}
              onClick={() => setActiveEnv(env.id)}
              size="sm"
              variant={env.id === selectedEnv ? "secondary" : "outline"}
            >
              {env.name}
            </Button>
          ))}
          <NewEnvironmentButton projectId={projectId} />
        </div>
      </div>

      {selectedEnv ? <EnvironmentApps environmentId={selectedEnv} /> : null}
    </section>
  );
}

function NewEnvironmentButton({ projectId }: { projectId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");

  const add = useMutation({
    mutationFn: () => createEnvironment(projectId, name),
    onSuccess: async () => {
      setName("");
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["environments", projectId] });
    },
  });

  if (!open) {
    return (
      <Button onClick={() => setOpen(true)} size="sm" variant="outline">
        + Environment
      </Button>
    );
  }

  return (
    <form
      className="flex items-center gap-2"
      onSubmit={(e) => {
        e.preventDefault();
        add.mutate();
      }}
    >
      <Input
        autoFocus
        className="h-8 w-40"
        onChange={(e) => setName(e.currentTarget.value)}
        placeholder="staging"
        value={name}
      />
      <Button loading={add.isPending} size="sm" type="submit">
        Add
      </Button>
    </form>
  );
}

function EnvironmentApps({ environmentId }: { environmentId: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["apps", environmentId];
  const apps = useQuery({ queryKey, queryFn: () => listApps(environmentId) });
  const servers = useQuery({ queryKey: ["servers", "for-apps"], queryFn: listServers });

  const [name, setName] = useState("");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [branch, setBranch] = useState("main");
  const [port, setPort] = useState("3000");
  const [serverId, setServerId] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () =>
      createApp({
        environmentId,
        name,
        repositoryUrl,
        branch,
        port: Number(port),
        serverId: serverId || undefined,
      }),
    onSuccess: async () => {
      setName("");
      setRepositoryUrl("");
      setBranch("main");
      setPort("3000");
      setServerId("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  const appList = apps.data ?? [];
  const serverList = servers.data ?? [];

  return (
    <div className="flex max-w-3xl flex-col gap-6">
      <div>
        {apps.isPending ? (
          <p className="text-muted-foreground text-sm">Loading apps…</p>
        ) : appList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No apps in this environment yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {appList.map((a) => (
              <li key={a.id}>
                <Link
                  to="/apps/$appId"
                  params={{ appId: a.id }}
                  className="flex items-center justify-between rounded-lg border bg-card px-4 py-3 hover:bg-accent/40"
                >
                  <span className="font-medium">{a.name}</span>
                  <span className="truncate font-mono text-muted-foreground text-xs">
                    {a.repositoryUrl}
                  </span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>

      <form className="space-y-4 rounded-lg border bg-card p-6" onSubmit={handleSubmit}>
        <h2 className="text-lg font-semibold">New app</h2>
        <div className="space-y-2">
          <Label htmlFor="app-name">Name</Label>
          <Input
            id="app-name"
            onChange={(e) => setName(e.currentTarget.value)}
            placeholder="web"
            required
            value={name}
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="app-repo">Repository URL</Label>
          <Input
            id="app-repo"
            onChange={(e) => setRepositoryUrl(e.currentTarget.value)}
            placeholder="https://github.com/user/repo"
            required
            value={repositoryUrl}
          />
        </div>
        <div className="flex gap-3">
          <div className="flex-1 space-y-2">
            <Label htmlFor="app-branch">Branch</Label>
            <Input
              id="app-branch"
              onChange={(e) => setBranch(e.currentTarget.value)}
              value={branch}
            />
          </div>
          <div className="w-28 space-y-2">
            <Label htmlFor="app-port">Port</Label>
            <Input
              id="app-port"
              onChange={(e) => setPort(e.currentTarget.value)}
              type="number"
              value={port}
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label>Server</Label>
          {servers.isPending ? (
            <p className="text-muted-foreground text-sm">Loading servers…</p>
          ) : serverList.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No servers yet —{" "}
              <Link to="/servers" className="font-medium text-foreground underline">
                add one
              </Link>{" "}
              first (you can also attach it after creating the app).
            </p>
          ) : (
            <Select value={serverId} onValueChange={(v) => setServerId(v ?? "")}>
              <SelectButton className="w-full">
                <SelectValue placeholder="Select a server">
                  {(value: string) =>
                    serverList.find((s) => s.id === value)?.name ?? "Select a server"
                  }
                </SelectValue>
              </SelectButton>
              <SelectPopup>
                {serverList.map((s) => (
                  <SelectItem key={s.id} value={s.id}>
                    {s.name}
                  </SelectItem>
                ))}
              </SelectPopup>
            </Select>
          )}
        </div>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button loading={add.isPending} type="submit">
          Create app
        </Button>
      </form>
    </div>
  );
}
