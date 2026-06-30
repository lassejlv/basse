import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { TrashIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import type { DeploymentStatus } from "@basse/shared";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import type { App } from "@/lib/apps";
import { getApp, updateApp } from "@/lib/apps";
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

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">{data.name}</h1>
        <p className="mt-2 font-mono text-muted-foreground text-sm">
          {data.repositoryUrl} · {data.branch} · :{data.port} · {data.buildMode}
        </p>
        {data.serverIds.length === 0 ? (
          <p className="mt-2 text-warning-foreground text-sm">
            No servers attached — select at least one before deploying.
          </p>
        ) : null}
      </div>

      <ServerCard app={data} />
      <DeploySection appId={appId} canDeploy={data.serverIds.length > 0} />
      <EnvVarsCard appId={appId} />
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

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  // Only this app's domains (the server may host domains for other apps too).
  const appDomains = (domains.data ?? []).filter((d) => d.appId === app.id);
  const selectedServer = (servers.data ?? []).find((s) => s.id === serverId);

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Domains</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Add an A record for your domain pointing to{" "}
        <code className="font-mono">{selectedServer?.sshHost ?? "this server"}</code>. Basse will
        configure HTTPS on that server and route traffic to the app.
      </p>

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
