import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon, CheckIcon, CopyIcon, RotateCcwIcon, TrashIcon } from "lucide-react";
import { FormEvent, type ReactNode, useEffect, useState } from "react";
import { chartCssVars } from "@/components/charts/chart-context";
import { Grid } from "@/components/charts/grid";
import { Line, LineChart } from "@/components/charts/line-chart";
import { ChartTooltip } from "@/components/charts/tooltip";
import { XAxis } from "@/components/charts/x-axis";
import { LogExplorer } from "@/components/log-explorer";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
import { createDomain, deleteDomain, listDomains, resyncProxy } from "@/lib/domains";
import { formatBytes, relativeTime } from "@/lib/format";
import {
  checkServerConnection,
  checkAgentUpdate,
  deleteServer,
  getAgentInfo,
  getAgentLogs,
  getAgentMetrics,
  getServer,
  getServerInstallCommand,
  provisionServer,
  sendServerDeleteCode,
  updateAgent,
} from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";

export const Route = createFileRoute("/_authed/servers/$serverId")({
  component: ServerDetailRoute,
});

function SectionHeading({ title, action }: { title: string; action?: ReactNode }) {
  return (
    <div className="mb-3 flex items-center justify-between gap-3">
      <h2 className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
        {title}
      </h2>
      {action}
    </div>
  );
}

function MetaTile({ label, value, mono }: { label: string; value: string; mono?: boolean }) {
  return (
    <div className="min-w-0 p-4">
      <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
        {label}
      </p>
      <p className={`mt-1 truncate text-sm ${mono ? "font-mono text-xs leading-5" : ""}`}>
        {value}
      </p>
    </div>
  );
}

function ServerDetailRoute() {
  const { serverId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const [deleteDialogOpen, setDeleteDialogOpen] = useState(false);
  const [deleteCode, setDeleteCode] = useState("");
  const [deleteError, setDeleteError] = useState<string | null>(null);

  const server = useQuery({
    queryKey: ["server", serverId],
    queryFn: () => getServer(serverId),
  });

  const remove = useMutation({
    mutationFn: () => deleteServer(serverId, deleteCode),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["servers", activeOrganization?.id] });
      toast.success("Server removed");
      navigate({ to: "/servers" });
    },
    onError: (error) => setDeleteError(toMessage(error)),
  });

  const requestDeleteCode = useMutation({
    mutationFn: () => sendServerDeleteCode(serverId),
    onSuccess: () => {
      setDeleteError(null);
      toast.success("Delete code sent");
    },
    onError: (error) => setDeleteError(toMessage(error)),
  });

  if (server.isPending) {
    return <p className="p-6 text-muted-foreground text-sm">Loading…</p>;
  }

  if (server.isError || !server.data) {
    return <p className="p-6 text-destructive-foreground text-sm">Server not found.</p>;
  }

  const data = server.data;

  return (
    <section className="flex flex-1 flex-col gap-6 p-4 md:p-6">
      <div>
        <Link
          className="inline-flex items-center gap-1.5 text-muted-foreground text-sm transition hover:text-foreground"
          to="/servers"
        >
          <ArrowLeftIcon className="size-4" />
          Servers
        </Link>
        <div className="mt-3 flex flex-wrap items-center gap-3">
          <h1 className="font-semibold text-2xl tracking-tight md:text-3xl">{data.name}</h1>
          <ServerStatusBadge status={data.status} />
          {data.isSystem ? (
            <Badge size="sm" variant="outline">
              Local
            </Badge>
          ) : null}
        </div>
        <p className="mt-1 font-mono text-muted-foreground text-sm">
          {data.connectionMode === "outbound"
            ? `outbound · ${data.sshHost}`
            : `${data.sshUser}@${data.sshHost}:${data.sshPort}`}
        </p>
        {data.statusMessage ? (
          <p className="mt-2 text-muted-foreground text-sm">{data.statusMessage}</p>
        ) : null}
      </div>

      <Card className="grid grid-cols-2 gap-0 divide-y p-0 sm:grid-cols-4 sm:divide-x sm:divide-y-0">
        <MetaTile
          label="Connection"
          value={data.connectionMode === "outbound" ? "Outbound agent" : "SSH"}
        />
        <MetaTile label="Host" mono value={data.sshHost} />
        <MetaTile
          label="Last seen"
          value={data.lastSeenAt ? relativeTime(data.lastSeenAt) : "Never"}
        />
        <MetaTile label="Added" value={relativeTime(data.createdAt)} />
      </Card>

      <div className="grid items-start gap-6 xl:grid-cols-[2fr_1fr]">
        <div className="flex min-w-0 flex-col gap-6">
          <AgentSection
            connectionMode={data.connectionMode}
            enabled={Boolean(data.agentTokenHint)}
            serverId={serverId}
          />
          <DomainsSection serverId={serverId} sshHost={data.sshHost} />
        </div>

        <div className="flex min-w-0 flex-col gap-6">
          {data.connectionMode === "outbound" ? (
            data.isSystem ? (
              <LocalServerSection />
            ) : (
              <OutboundInstallSection serverId={serverId} />
            )
          ) : (
            <SshSetupSection data={data} serverId={serverId} />
          )}

          {!data.isSystem ? (
            <section>
              <SectionHeading title="Danger zone" />
              <Card className="flex flex-row items-center justify-between gap-3 border-destructive/24 p-4">
                <p className="min-w-0 text-muted-foreground text-xs">
                  Removes the server and discards its access key. Running containers are left in
                  place.
                </p>
                <Button
                  className="shrink-0"
                  onClick={() => setDeleteDialogOpen(true)}
                  size="sm"
                  variant="destructive-outline"
                >
                  <TrashIcon />
                  Delete
                </Button>
              </Card>
            </section>
          ) : null}
        </div>
      </div>

      <Dialog
        open={deleteDialogOpen}
        onOpenChange={(open) => {
          setDeleteDialogOpen(open);
          if (!open) {
            setDeleteCode("");
            setDeleteError(null);
          }
        }}
      >
        <DialogPopup>
          <DialogHeader>
            <DialogTitle>Delete server</DialogTitle>
            <DialogDescription>
              Send a confirmation code to your email, then enter it to delete {data.name}.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <Button
              loading={requestDeleteCode.isPending}
              onClick={() => requestDeleteCode.mutate()}
              type="button"
              variant="outline"
            >
              Send code
            </Button>
            <div className="space-y-2">
              <Label htmlFor="server-delete-code">Confirmation code</Label>
              <Input
                autoComplete="one-time-code"
                id="server-delete-code"
                inputMode="numeric"
                maxLength={6}
                onChange={(event) =>
                  setDeleteCode(event.currentTarget.value.replace(/\D/g, "").slice(0, 6))
                }
                placeholder="000000"
                value={deleteCode}
              />
            </div>
            {deleteError ? (
              <p className="text-destructive-foreground text-sm">{deleteError}</p>
            ) : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose
              render={
                <Button type="button" variant="outline">
                  Cancel
                </Button>
              }
            />
            <Button
              disabled={deleteCode.length !== 6}
              loading={remove.isPending}
              onClick={() => remove.mutate()}
              type="button"
              variant="destructive"
            >
              Confirm delete
            </Button>
          </DialogFooter>
        </DialogPopup>
      </Dialog>
    </section>
  );
}

function SshSetupSection({
  serverId,
  data,
}: {
  serverId: string;
  data: NonNullable<Awaited<ReturnType<typeof getServer>>>;
}) {
  const queryClient = useQueryClient();
  const [copied, setCopied] = useState(false);

  const test = useMutation({
    mutationFn: () => checkServerConnection(serverId),
    onSuccess: async (result) => {
      await queryClient.invalidateQueries({ queryKey: ["server", serverId] });
      if (result.ok) {
        toast.success("Connection OK");
      } else {
        toast.error("Couldn't connect", { description: result.error ?? "Server unreachable" });
      }
    },
    onError: (error) => {
      toast.error("Couldn't test connection", { description: toMessage(error) });
    },
  });

  const provision = useMutation({
    mutationFn: () => provisionServer(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["server", serverId] });
      toast.success("Provisioning started");
    },
  });

  async function copyPublicKey() {
    await navigator.clipboard.writeText(data.sshPublicKey);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  return (
    <section>
      <SectionHeading title="Setup" />
      <Card className="p-5">
        <h3 className="font-medium text-sm">Install the access key</h3>
        <p className="mt-1 text-muted-foreground text-sm">
          Add this public key to <code className="font-mono">~/.ssh/authorized_keys</code> on the
          server (as <code className="font-mono">{data.sshUser}</code>), then provision it.
        </p>
        <pre className="mt-3 max-h-32 overflow-y-auto whitespace-pre-wrap break-all rounded-lg border bg-muted/30 p-3 font-mono text-xs">
          {data.sshPublicKey}
        </pre>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button onClick={copyPublicKey} size="sm" variant="outline">
            {copied ? <CheckIcon /> : <CopyIcon />}
            {copied ? "Copied" : "Copy key"}
          </Button>
          <Button
            loading={test.isPending}
            onClick={() => test.mutate()}
            size="sm"
            variant="outline"
          >
            Test connection
          </Button>
          {test.data ? (
            test.data.ok ? (
              <span className="text-success-foreground text-sm">Reachable</span>
            ) : (
              <span className="text-destructive-foreground text-sm">
                {test.data.error ?? "Unreachable"}
              </span>
            )
          ) : null}
        </div>

        <div className="mt-5 border-t pt-4">
          <h3 className="font-medium text-sm">Provisioning</h3>
          <p className="mt-1 text-muted-foreground text-sm">
            Installs Docker (if missing) and runs the Basse agent over SSH. Safe to run again.
          </p>
          {data.status === "active" ? (
            <p className="mt-2 text-success-foreground text-sm">
              Agent active
              {data.lastSeenAt
                ? ` · last seen ${new Date(data.lastSeenAt).toLocaleString()}`
                : null}
            </p>
          ) : null}
          <div className="mt-3 flex items-center gap-2">
            <Button
              disabled={data.status === "provisioning"}
              loading={provision.isPending || data.status === "provisioning"}
              onClick={() => provision.mutate()}
              size="sm"
            >
              {data.status === "active" || data.status === "unreachable"
                ? "Re-provision"
                : "Provision"}
            </Button>
            {provision.isError ? (
              <span className="text-destructive-foreground text-sm">
                {(provision.error as Error).message}
              </span>
            ) : null}
          </div>
        </div>
      </Card>
    </section>
  );
}

function OutboundInstallSection({ serverId }: { serverId: string }) {
  const [command, setCommand] = useState("");

  const install = useMutation({
    mutationFn: () => getServerInstallCommand(serverId),
    onSuccess: (result) => setCommand(result.agentInstallCommand),
    onError: (error) => {
      toast.error("Couldn't load install command", { description: toMessage(error) });
    },
  });

  async function copyCommand() {
    if (!command) return;
    await navigator.clipboard.writeText(command);
    toast.success("Install command copied");
  }

  return (
    <section>
      <SectionHeading title="Setup" />
      <Card className="p-5">
        <h3 className="font-medium text-sm">Outbound agent</h3>
        <p className="mt-1 text-muted-foreground text-sm">
          Experimental mode for servers where SSH is not possible. SSH mode is recommended when the
          server can accept it.
        </p>
        <p className="mt-2 text-muted-foreground text-sm">
          Run the install command on the server. The token is embedded in the command and should be
          treated like a secret.
        </p>
        <div className="mt-3 flex flex-wrap items-center gap-2">
          <Button
            loading={install.isPending}
            onClick={() => install.mutate()}
            size="sm"
            variant="outline"
          >
            {command ? "Refresh command" : "Show install command"}
          </Button>
          {command ? (
            <Button onClick={copyCommand} size="sm" type="button">
              <CopyIcon />
              Copy command
            </Button>
          ) : null}
        </div>
        {command ? (
          <pre className="mt-3 max-h-80 overflow-auto whitespace-pre-wrap break-all rounded-lg border bg-muted/30 p-3 font-mono text-xs">
            {command}
          </pre>
        ) : (
          <p className="mt-3 text-muted-foreground text-sm">
            The server becomes active after the installed agent reaches Basse for the first time.
          </p>
        )}
      </Card>
    </section>
  );
}

function LocalServerSection() {
  return (
    <section>
      <SectionHeading title="Setup" />
      <Card className="p-5">
        <h3 className="font-medium text-sm">Local self-host server</h3>
        <p className="mt-1 text-muted-foreground text-sm">
          This target is created automatically for self-hosted installs and uses the bundled local
          agent container.
        </p>
        <p className="mt-2 text-muted-foreground text-sm">
          It deploys workloads onto the Docker host running Basse and cannot be deleted.
        </p>
      </Card>
    </section>
  );
}

function AgentSection({
  serverId,
  enabled,
  connectionMode,
}: {
  serverId: string;
  enabled: boolean;
  connectionMode: "ssh" | "outbound";
}) {
  const queryClient = useQueryClient();
  const supportsHostAgentOps = connectionMode === "ssh";
  const [samples, setSamples] = useState<
    {
      date: Date;
      cpuPercent: number;
      memoryPercent: number;
      memoryBytes: number;
      memoryLimitBytes: number;
    }[]
  >([]);

  const info = useQuery({
    queryKey: ["agent-info", serverId],
    queryFn: () => getAgentInfo(serverId),
    enabled,
    refetchInterval: 10000,
  });

  const metrics = useQuery({
    queryKey: ["agent-metrics", serverId],
    queryFn: () => getAgentMetrics(serverId),
    enabled: enabled && supportsHostAgentOps,
    refetchInterval: 5000,
  });

  const logs = useQuery({
    queryKey: ["agent-logs", serverId],
    queryFn: () => getAgentLogs(serverId),
    enabled: enabled && supportsHostAgentOps,
    refetchInterval: 10000,
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

  const checkUpdate = useMutation({
    mutationFn: () => checkAgentUpdate(serverId),
    onSuccess: (result) => {
      toast.success(result.updateAvailable ? "Update available" : "Agent is up to date");
    },
  });
  const runUpdate = useMutation({
    mutationFn: () => updateAgent(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["server", serverId] });
      await queryClient.invalidateQueries({ queryKey: ["agent-info", serverId] });
      toast.success("Agent updated");
    },
    onError: (error) => {
      toast.error("Couldn't update agent", { description: toMessage(error) });
    },
  });

  return (
    <section>
      <SectionHeading
        action={
          <Badge size="sm" variant={info.data?.ready ? "success" : "outline"}>
            {info.data?.ready ? "ready" : "unknown"}
          </Badge>
        }
        title="Agent"
      />
      <Card className="p-5">
        {!enabled ? (
          <p className="rounded-lg border border-dashed px-3 py-6 text-center text-muted-foreground text-sm">
            Provision this server to install the agent.
          </p>
        ) : (
          <>
            <dl className="grid grid-cols-2 gap-x-4 gap-y-3 sm:grid-cols-4">
              <div className="min-w-0">
                <dt className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
                  Version
                </dt>
                <dd className="mt-0.5 truncate font-mono text-xs leading-5">
                  {info.data?.version ?? "unknown"}
                </dd>
              </div>
              <div className="min-w-0">
                <dt className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
                  Target image
                </dt>
                <dd
                  className="mt-0.5 truncate font-mono text-xs leading-5"
                  title={info.data?.targetImage}
                >
                  {info.data?.targetImage ?? "unknown"}
                </dd>
              </div>
              <div className="min-w-0">
                <dt className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
                  Docker
                </dt>
                <dd className="mt-0.5 truncate text-sm">
                  {info.data?.docker
                    ? `${info.data.docker.containersRunning}/${info.data.docker.containers} running`
                    : "unknown"}
                </dd>
              </div>
              <div className="min-w-0">
                <dt className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
                  Engine
                </dt>
                <dd className="mt-0.5 truncate text-sm">
                  {info.data?.engine?.version ?? "unknown"}
                </dd>
              </div>
            </dl>

            {supportsHostAgentOps ? (
              <>
                <div className="mt-4 overflow-hidden rounded-lg border">
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
                    <Line
                      dataKey="memoryPercent"
                      stroke={chartCssVars.lineSecondary}
                      strokeWidth={2}
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
                          value: `${formatBytes(Number(point.memoryBytes))} / ${formatBytes(
                            Number(point.memoryLimitBytes),
                          )}`,
                        },
                      ]}
                    />
                  </LineChart>
                  <div className="flex items-center gap-4 border-t px-3 py-2 text-muted-foreground text-xs">
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
                    <span className="ml-auto flex items-center gap-2">
                      <Button
                        loading={checkUpdate.isPending}
                        onClick={() => checkUpdate.mutate()}
                        size="xs"
                        variant="outline"
                      >
                        Check updates
                      </Button>
                      <Button
                        loading={runUpdate.isPending}
                        onClick={() => runUpdate.mutate()}
                        size="xs"
                      >
                        Update agent
                      </Button>
                    </span>
                  </div>
                </div>
                {checkUpdate.data ? (
                  <p className="mt-2 text-muted-foreground text-sm">
                    {checkUpdate.data.updateAvailable
                      ? "Update available"
                      : "Agent image is current"}
                  </p>
                ) : null}
                {checkUpdate.isError ? (
                  <p className="mt-2 text-destructive-foreground text-sm">
                    {(checkUpdate.error as Error).message}
                  </p>
                ) : null}

                <div className="mt-4">
                  <h3 className="mb-2 font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
                    Agent logs
                  </h3>
                  <LogExplorer
                    downloadName="basse-agent.log"
                    emptyText={logs.isPending ? "Loading logs…" : "No logs yet."}
                    isRefreshing={logs.isFetching}
                    live={!logs.isError}
                    maxHeight="18rem"
                    onRefresh={() => void logs.refetch()}
                    text={logs.data?.logs ?? ""}
                  />
                </div>
              </>
            ) : (
              <p className="mt-4 text-muted-foreground text-sm">
                Outbound agents report health and Docker info here. Host-level agent logs, host
                metrics, and automatic agent updates require SSH.
              </p>
            )}
          </>
        )}
      </Card>
    </section>
  );
}

const DOMAIN_STATUS_VARIANT = {
  pending: "outline",
  active: "success",
  error: "error",
} as const;

function DomainsSection({ serverId, sshHost }: { serverId: string; sshHost: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["domains", serverId];
  const [host, setHost] = useState("");
  const [upstream, setUpstream] = useState("");
  const [error, setError] = useState<string | null>(null);

  const domains = useQuery({
    queryKey,
    queryFn: () => listDomains(serverId),
  });

  const add = useMutation({
    mutationFn: () => createDomain(serverId, { host, upstream }),
    onSuccess: async () => {
      setHost("");
      setUpstream("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Domain added");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteDomain(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Domain removed");
    },
    onError: (error) => {
      toast.error("Couldn't remove domain", { description: toMessage(error) });
    },
  });

  const resync = useMutation({
    mutationFn: () => resyncProxy(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Proxy resynced");
    },
    onError: (error) => {
      toast.error("Couldn't resync proxy", { description: toMessage(error) });
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  const domainList = domains.data ?? [];

  return (
    <section>
      <SectionHeading
        action={
          <Button
            loading={resync.isPending}
            onClick={() => resync.mutate()}
            size="xs"
            variant="outline"
          >
            <RotateCcwIcon />
            Resync proxy
          </Button>
        }
        title="Domains"
      />
      <Card className="p-5">
        <p className="text-muted-foreground text-sm">
          Point each domain's DNS A record at <code className="font-mono">{sshHost}</code>, then it
          is routed to its upstream with automatic HTTPS.
        </p>

        <div className="mt-4">
          {domains.isPending ? (
            <div className="h-16 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
          ) : domainList.length === 0 ? (
            <p className="rounded-lg border border-dashed px-3 py-5 text-center text-muted-foreground text-sm">
              No domains routed on this server yet.
            </p>
          ) : (
            <ul className="divide-y rounded-lg border">
              {domainList.map((d) => (
                <li key={d.id} className="flex items-center justify-between gap-3 px-3 py-2">
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <p className="truncate font-medium text-sm">{d.host}</p>
                      <Badge size="sm" variant={DOMAIN_STATUS_VARIANT[d.status]}>
                        {d.status}
                      </Badge>
                    </div>
                    <p className="truncate font-mono text-muted-foreground text-xs">
                      → {d.upstream}
                      {d.statusMessage ? ` · ${d.statusMessage}` : ""}
                    </p>
                  </div>
                  <Button
                    aria-label={`Delete ${d.host}`}
                    loading={remove.isPending && remove.variables === d.id}
                    onClick={() => remove.mutate(d.id)}
                    size="icon-sm"
                    variant="ghost"
                  >
                    <TrashIcon />
                  </Button>
                </li>
              ))}
            </ul>
          )}
        </div>

        <form
          className="mt-5 grid gap-3 border-t pt-4 sm:grid-cols-[1fr_1fr_auto]"
          onSubmit={handleSubmit}
        >
          <div className="space-y-2">
            <Label htmlFor="domain-host">Domain</Label>
            <Input
              id="domain-host"
              onChange={(event) => setHost(event.currentTarget.value)}
              placeholder="app.example.com"
              required
              value={host}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="domain-upstream">Upstream</Label>
            <Input
              id="domain-upstream"
              onChange={(event) => setUpstream(event.currentTarget.value)}
              placeholder="my-container:3000"
              required
              value={upstream}
            />
          </div>
          <div className="flex items-end">
            <Button loading={add.isPending} type="submit">
              Add domain
            </Button>
          </div>
          {error ? (
            <p className="text-destructive-foreground text-sm sm:col-span-3">{error}</p>
          ) : null}
        </form>
      </Card>
    </section>
  );
}
