import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { PlusIcon, RotateCcwIcon, TrashIcon } from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import type { LoadBalancerEvent, LoadBalancerIntegration, ManagedLoadBalancer } from "@basse/shared";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import type { App } from "@/lib/apps";
import type { AppStagedChanges, StagedChange } from "@/lib/changes";
import { getPreviewDomainConfig, stageDomainChange, stagePreviewDomain } from "@/lib/changes";
import { listDomains, resyncProxy, type Domain } from "@/lib/domains";
import {
  createManagedLoadBalancer,
  deleteManagedLoadBalancer,
  listLoadBalancerEvents,
  listLoadBalancerIntegrations,
  listManagedLoadBalancers,
  syncManagedLoadBalancer,
} from "@/lib/load-balancers";
import { listServers } from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";

/** Domains routing for an app: direct Caddy domains on a single server, or a
 * managed load balancer across several. */
export function AppDomainsTab({
  app,
  draft,
  projectId,
  stagedChanges,
}: {
  app: App;
  draft: App;
  projectId?: string;
  stagedChanges: StagedChange[];
}) {
  if (app.serverIds.length === 1) {
    return (
      <AppDomainsSection
        app={draft}
        projectId={projectId}
        serverId={app.serverIds[0]!}
        stagedChanges={stagedChanges}
      />
    );
  }
  if (app.serverIds.length > 1) {
    return <ManagedLoadBalancerSection app={app} />;
  }
  return (
    <Card className="p-6">
      <p className="text-muted-foreground text-sm">
        Attach a server to this app to route a domain to it.
      </p>
    </Card>
  );
}

type StagedDomainPayload = {
  id?: string | null;
  serverId?: string;
  appId?: string | null;
  host?: string;
  upstream?: string;
};

type DomainDisplayItem = Domain & {
  pendingAction?: "create" | "update" | "delete";
};

function parseStagedDomain(raw: string | null): StagedDomainPayload | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as StagedDomainPayload;
  } catch {
    return null;
  }
}

function domainField(serverId: string, host: string): string {
  return `${serverId}:${host}`;
}

async function cacheDomainStage(
  queryClient: ReturnType<typeof useQueryClient>,
  appId: string,
  projectId: string | undefined,
  data: AppStagedChanges,
) {
  queryClient.setQueryData(["changes", appId], data);
  await Promise.all([
    queryClient.invalidateQueries({ queryKey: ["changes", appId] }),
    queryClient.invalidateQueries({ queryKey: ["change-history", appId] }),
    queryClient.invalidateQueries({ queryKey: ["preview-domain", appId] }),
    ...(projectId
      ? [
          queryClient.invalidateQueries({ queryKey: ["project-changes", projectId] }),
          queryClient.invalidateQueries({ queryKey: ["project-change-history", projectId] }),
        ]
      : []),
  ]);
}

function AppDomainsSection({
  app,
  projectId,
  serverId,
  stagedChanges,
}: {
  app: App;
  projectId?: string;
  serverId: string;
  stagedChanges: StagedChange[];
}) {
  const queryClient = useQueryClient();
  const queryKey = ["domains", serverId];
  const upstream = `basse-app-${app.id}:${app.port}`;
  const servers = useQuery({ queryKey: ["servers", "for-domains"], queryFn: listServers });
  const [host, setHost] = useState("");
  const [error, setError] = useState<string | null>(null);

  const domains = useQuery({
    queryKey,
    queryFn: () => listDomains(serverId),
  });
  const previewConfig = useQuery({
    queryKey: ["preview-domain", app.id],
    queryFn: () => getPreviewDomainConfig(app.id),
  });

  const add = useMutation({
    mutationFn: () =>
      stageDomainChange(app.id, {
        action: "create",
        serverId,
        host,
        upstream,
      }),
    onSuccess: async (data) => {
      setHost("");
      setError(null);
      toast.success("Domain change staged");
      await cacheDomainStage(queryClient, app.id, projectId, data);
    },
    onError: (e: Error) => setError(e.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => stageDomainChange(app.id, { action: "delete", domainId: id }),
    onSuccess: async (data) => {
      toast.success("Domain change staged");
      await cacheDomainStage(queryClient, app.id, projectId, data);
    },
    onError: (e: Error) => toast.error("Couldn't remove domain", { description: toMessage(e) }),
  });

  const addPreview = useMutation({
    mutationFn: () => {
      if (previewConfig.data?.enabled) {
        return stagePreviewDomain(app.id);
      }
      const ip = selectedServer?.sshHost ?? "";
      const previewDomainHost = `${app.slug}-${app.id.slice(0, 8)}.${ip}.sslip.io`;
      return stageDomainChange(app.id, {
        action: "create",
        serverId,
        host: previewDomainHost,
        upstream,
      });
    },
    onSuccess: async (data) => {
      setError(null);
      toast.success("Preview domain staged");
      await cacheDomainStage(queryClient, app.id, projectId, data);
      await queryClient.invalidateQueries({ queryKey: ["preview-domain", app.id] });
    },
    onError: (e: Error) => setError(e.message),
  });

  const resync = useMutation({
    mutationFn: () => resyncProxy(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Proxy sync queued");
    },
    onError: (resyncError) => {
      toast.error("Couldn't sync proxy", { description: toMessage(resyncError) });
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  // Only this app's domains (the server may host domains for other apps too).
  const stagedDomainChanges = stagedChanges.filter((change) => change.resource === "domain");
  const stagedDeleteFields = new Set(
    stagedDomainChanges
      .filter((change) => change.action === "delete")
      .map((change) => change.field),
  );
  const stagedUpdateFields = new Map<string, StagedDomainPayload>();
  for (const change of stagedDomainChanges) {
    if (change.action !== "update") continue;
    const value = parseStagedDomain(change.value);
    if (value) stagedUpdateFields.set(change.field, value);
  }
  const stagedCreates: DomainDisplayItem[] = stagedDomainChanges.flatMap((change) => {
    if (change.action !== "create" || !change.value) return [];
    const value = parseStagedDomain(change.value);
    if (!value?.serverId || !value.host || !value.upstream || value.serverId !== serverId) {
      return [];
    }
    return [
      {
        id: change.id,
        serverId: value.serverId,
        appId: app.id,
        host: value.host,
        upstream: value.upstream,
        status: "pending",
        statusMessage: null,
        pendingAction: "create",
        createdAt: change.createdAt,
        updatedAt: change.createdAt,
      },
    ];
  });
  const appDomains: DomainDisplayItem[] = (domains.data ?? [])
    .filter((d) => d.appId === app.id)
    .map((d) => {
      const field = domainField(d.serverId, d.host);
      const stagedUpdate = stagedUpdateFields.get(field);
      return {
        ...d,
        upstream: stagedUpdate?.upstream ?? d.upstream,
        pendingAction: stagedDeleteFields.has(field)
          ? ("delete" as const)
          : stagedUpdate
            ? ("update" as const)
            : undefined,
      };
    });
  const visibleDomains = [
    ...appDomains,
    ...stagedCreates.filter(
      (staged) =>
        !appDomains.some(
          (live) =>
            domainField(live.serverId, live.host) === domainField(staged.serverId, staged.host),
        ),
    ),
  ];
  const selectedServer = (servers.data ?? []).find((s) => s.id === serverId);
  const cloudPreviewEnabled = previewConfig.data?.enabled === true;
  const canGenerateSslipPreview = Boolean(
    selectedServer?.sshHost.match(/^\d{1,3}(?:\.\d{1,3}){3}$/),
  );
  const sslipPreviewHost = selectedServer
    ? `${app.slug}-${app.id.slice(0, 8)}.${selectedServer.sshHost}.sslip.io`
    : "";
  const previewHost = previewConfig.isPending
    ? ""
    : cloudPreviewEnabled
      ? (previewConfig.data?.host ?? "")
      : sslipPreviewHost;
  const canGeneratePreview =
    !previewConfig.isPending && (cloudPreviewEnabled || canGenerateSslipPreview);
  const previewUnavailableText = previewConfig.isPending
    ? "Loading..."
    : cloudPreviewEnabled
      ? `Waiting for ${previewConfig.data?.rootDomain ?? "preview domain"}`
      : "Requires an IPv4 server address";

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Domains</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Add an A record for your domain pointing to{" "}
            <code className="font-mono">{selectedServer?.sshHost ?? "this server"}</code>. Basse
            will configure HTTPS on that server and route traffic to the app.
          </p>
        </div>
        <Button
          loading={resync.isPending}
          onClick={() => resync.mutate()}
          size="sm"
          variant="outline"
        >
          <RotateCcwIcon />
          Sync
        </Button>
      </div>
      <div className="mt-4 rounded-md border bg-muted/20 p-3">
        <div className="flex flex-wrap items-center justify-between gap-3">
          <div className="min-w-0">
            <p className="font-medium text-sm">Preview domain</p>
            <p className="truncate font-mono text-muted-foreground text-xs">
              {previewHost || previewUnavailableText}
            </p>
          </div>
          <Button
            disabled={!canGeneratePreview}
            loading={addPreview.isPending}
            onClick={() => addPreview.mutate()}
            size="sm"
            variant="outline"
          >
            {cloudPreviewEnabled ? "Generate preview" : "Generate sslip.io"}
          </Button>
        </div>
      </div>

      <div className="mt-5">
        {visibleDomains.length === 0 ? (
          <p className="text-muted-foreground text-sm">No domains yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {visibleDomains.map((d) => (
              <li
                key={d.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
              >
                <span className="flex min-w-0 items-center gap-2">
                  <Badge
                    size="sm"
                    variant={
                      d.pendingAction
                        ? "outline"
                        : d.status === "active"
                          ? "success"
                          : d.status === "error"
                            ? "error"
                            : "warning"
                    }
                  >
                    {d.pendingAction === "create"
                      ? "staged"
                      : d.pendingAction === "delete"
                        ? "staged remove"
                        : d.pendingAction === "update"
                          ? "staged update"
                          : d.status}
                  </Badge>
                  <span className="min-w-0">
                    <span className="block truncate font-medium text-sm">{d.host}</span>
                    {d.statusMessage ? (
                      <span
                        className="block truncate text-muted-foreground text-xs"
                        title={d.statusMessage}
                      >
                        {d.statusMessage}
                      </span>
                    ) : null}
                  </span>
                </span>
                {d.pendingAction ? null : (
                  <Button
                    aria-label={`Delete ${d.host}`}
                    loading={remove.isPending && remove.variables === d.id}
                    onClick={() => remove.mutate(d.id)}
                    size="icon"
                    variant="outline"
                  >
                    <TrashIcon />
                  </Button>
                )}
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
          Stage
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

function providerOptionLabel(integration: LoadBalancerIntegration | null): string {
  if (!integration) return "Provider";
  return `${integration.name} · ${integration.provider}`;
}

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
  const integrationList = integrations.data ?? [];
  const selectedIntegration =
    integrationList.find((integration) => integration.id === integrationId) ?? null;
  const selectedProvider = selectedIntegration?.provider ?? "hetzner";

  useEffect(() => {
    const firstIntegration = integrations.data?.[0]?.id ?? "";
    if (!integrationId && firstIntegration) {
      setIntegrationId(firstIntegration);
    }
  }, [integrationId, integrations.data]);

  useEffect(() => {
    if (selectedProvider === "cloudflare") {
      setLocation("auto-zone");
      setLoadBalancerType("proxied");
      return;
    }

    if (location === "auto-zone") setLocation("fsn1");
    if (loadBalancerType === "proxied") setLoadBalancerType("lb11");
  }, [loadBalancerType, location, selectedProvider]);

  const create = useMutation({
    mutationFn: () =>
      createManagedLoadBalancer({
        appId: app.id,
        integrationId,
        host,
        location: selectedProvider === "hetzner" ? location : "auto-zone",
        loadBalancerType: selectedProvider === "hetzner" ? loadBalancerType : "proxied",
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
            Connect Hetzner or Cloudflare in Settings before creating a managed load balancer.
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
              <Select
                value={integrationId}
                onValueChange={(value) => setIntegrationId(value ?? "")}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Provider">
                    {(value: string) =>
                      providerOptionLabel(
                        integrationList.find((integration) => integration.id === value) ?? null,
                      )
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  {integrationList.map((integration) => (
                    <SelectItem key={integration.id} value={integration.id}>
                      {providerOptionLabel(integration)}
                    </SelectItem>
                  ))}
                </SelectPopup>
              </Select>
            </div>
          </div>
          <div className={`grid gap-3 ${selectedProvider === "hetzner" ? "sm:grid-cols-3" : ""}`}>
            {selectedProvider === "hetzner" ? (
              <>
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
              </>
            ) : (
              <div className="rounded-md border bg-muted/20 p-3 text-muted-foreground text-sm">
                Basse will resolve the Cloudflare zone from the domain, then create a proxied load
                balancer with one pool and health monitor.
              </div>
            )}
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
            {selectedProvider === "hetzner"
              ? "Hetzner creates a Basse-owned load balancer with TCP passthrough on 80 and 443, so each target server's Caddy keeps handling TLS and app routing."
              : "Cloudflare creates the public hostname directly in your zone. Point the domain to Cloudflare nameservers first."}
          </p>
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          <Button
            disabled={!integrationId || !host.trim()}
            loading={create.isPending}
            type="submit"
          >
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
  const eventsKey = ["load-balancer-events", loadBalancer.id];
  const events = useQuery({
    queryKey: eventsKey,
    queryFn: () => listLoadBalancerEvents(loadBalancer.id),
  });
  const sync = useMutation({
    mutationFn: () => syncManagedLoadBalancer(loadBalancer.id),
    onSuccess: async (updated) => {
      await Promise.all([
        queryClient.invalidateQueries({ queryKey }),
        queryClient.invalidateQueries({ queryKey: eventsKey }),
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
        queryClient.invalidateQueries({ queryKey: eventsKey }),
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

      {loadBalancer.provider === "cloudflare" ? (
        <div className="mt-4 rounded-md border bg-background p-3 text-sm">
          <p className="font-medium">Cloudflare DNS</p>
          <p className="mt-1 text-muted-foreground">
            {loadBalancer.host} is managed as a proxied Cloudflare load balancer in the matching
            zone.
          </p>
        </div>
      ) : (
        <div className="mt-4 grid gap-3 text-sm sm:grid-cols-2">
          <DnsRecord label="A record" value={loadBalancer.endpointIpv4} host={loadBalancer.host} />
          <DnsRecord
            label="AAAA record"
            value={loadBalancer.endpointIpv6}
            host={loadBalancer.host}
          />
        </div>
      )}

      <div className="mt-4">
        <p className="font-medium text-sm">Target health</p>
        {loadBalancer.targets.length === 0 ? (
          <p className="mt-1 text-muted-foreground text-sm">No targets synced yet.</p>
        ) : (
          <ul className="mt-2 flex flex-col gap-2">
            {loadBalancer.targets.map((target) => (
              <li
                key={target.id}
                className="flex items-center justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-sm"
              >
                <span className="min-w-0">
                  <span className="block truncate font-mono text-xs">{target.address}</span>
                  {target.statusMessage ? (
                    <span className="mt-1 block truncate text-muted-foreground text-xs">
                      {target.statusMessage}
                    </span>
                  ) : null}
                </span>
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

      <LoadBalancerEventList events={events.data ?? []} loading={events.isPending} />

      {loadBalancer.lastSyncedAt ? (
        <p className="mt-4 text-muted-foreground text-xs">
          Last synced {new Date(loadBalancer.lastSyncedAt).toLocaleString()}
        </p>
      ) : null}
    </div>
  );
}

function LoadBalancerEventList({
  events,
  loading,
}: {
  events: LoadBalancerEvent[];
  loading: boolean;
}) {
  return (
    <div className="mt-4">
      <p className="font-medium text-sm">Recent activity</p>
      {loading ? (
        <p className="mt-1 text-muted-foreground text-sm">Loading activity...</p>
      ) : events.length === 0 ? (
        <p className="mt-1 text-muted-foreground text-sm">No activity recorded yet.</p>
      ) : (
        <ul className="mt-2 flex flex-col gap-2">
          {events.slice(0, 6).map((event) => (
            <li
              className="flex items-start justify-between gap-3 rounded-md border bg-muted/20 px-3 py-2 text-sm"
              key={event.id}
            >
              <span className="min-w-0">
                <span className="block truncate">{event.message}</span>
                {event.details ? (
                  <span className="mt-1 block truncate text-muted-foreground text-xs">
                    {event.details}
                  </span>
                ) : null}
              </span>
              <span className="flex shrink-0 items-center gap-2">
                <span className="text-muted-foreground text-xs">
                  {new Date(event.createdAt).toLocaleString()}
                </span>
                <Badge
                  size="sm"
                  variant={
                    event.status === "success"
                      ? "success"
                      : event.status === "error"
                        ? "error"
                        : "outline"
                  }
                >
                  {event.status}
                </Badge>
              </span>
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}

function DnsRecord({ label, value, host }: { label: string; value: string | null; host: string }) {
  return (
    <div className="rounded-md border bg-background p-3">
      <p className="text-muted-foreground">{label}</p>
      <p className="mt-1 truncate font-mono text-xs">
        {value ? `${host} -> ${value}` : "Waiting for provider endpoint"}
      </p>
    </div>
  );
}
