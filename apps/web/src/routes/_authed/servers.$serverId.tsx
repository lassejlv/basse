import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon, TrashIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
import {
  createDomain,
  deleteDomain,
  listDomains,
  resyncProxy,
} from "@/lib/domains";
import {
  checkServerConnection,
  deleteServer,
  getServer,
  provisionServer,
} from "@/lib/servers";

export const Route = createFileRoute("/_authed/servers/$serverId")({
  component: ServerDetailRoute,
});

function ServerDetailRoute() {
  const { serverId } = Route.useParams();
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const [copied, setCopied] = useState(false);

  const server = useQuery({
    queryKey: ["server", serverId],
    queryFn: () => getServer(serverId),
    // Poll while a provision is in flight; stop once it reaches a terminal state.
    refetchInterval: (query) => {
      const status = query.state.data?.status;
      return status === "pending" || status === "provisioning" ? 2000 : false;
    },
  });

  const remove = useMutation({
    mutationFn: () => deleteServer(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["servers", activeOrganization?.id] });
      navigate({ to: "/servers" });
    },
  });

  const test = useMutation({
    mutationFn: () => checkServerConnection(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["server", serverId] });
    },
  });

  const provision = useMutation({
    mutationFn: () => provisionServer(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["server", serverId] });
    },
  });

  async function copyPublicKey() {
    if (!server.data) {
      return;
    }
    await navigator.clipboard.writeText(server.data.sshPublicKey);
    setCopied(true);
    setTimeout(() => setCopied(false), 1500);
  }

  if (server.isPending) {
    return <p className="p-6 text-muted-foreground text-sm">Loading…</p>;
  }

  if (server.isError || !server.data) {
    return <p className="p-6 text-destructive-foreground text-sm">Server not found.</p>;
  }

  const data = server.data;

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <Link
          to="/servers"
          className="inline-flex items-center gap-1.5 text-muted-foreground text-sm hover:text-foreground"
        >
          <ArrowLeftIcon className="size-4" />
          Servers
        </Link>
        <div className="mt-3 flex items-center gap-3">
          <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">{data.name}</h1>
          <ServerStatusBadge status={data.status} />
        </div>
        <p className="mt-2 font-mono text-muted-foreground text-sm">
          {data.sshUser}@{data.sshHost}:{data.sshPort}
        </p>
        {data.statusMessage ? (
          <p className="mt-2 text-muted-foreground text-sm">{data.statusMessage}</p>
        ) : null}
      </div>

      <div className="max-w-2xl rounded-lg border bg-card p-6">
        <h2 className="text-lg font-semibold">Install the access key</h2>
        <p className="mt-1 text-muted-foreground text-sm">
          Add this public key to <code className="font-mono">~/.ssh/authorized_keys</code> on the
          server (as <code className="font-mono">{data.sshUser}</code>), then provision it.
        </p>
        <pre className="mt-4 overflow-x-auto rounded-md border bg-muted/40 p-3 font-mono text-xs">
          {data.sshPublicKey}
        </pre>
        <div className="mt-3 flex items-center gap-2">
          <Button onClick={copyPublicKey} size="sm" variant="outline">
            {copied ? "Copied" : "Copy key"}
          </Button>
          <Button loading={test.isPending} onClick={() => test.mutate()} size="sm" variant="outline">
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
      </div>

      <div className="max-w-2xl rounded-lg border bg-card p-6">
        <h2 className="text-lg font-semibold">Provisioning</h2>
        <p className="mt-1 text-muted-foreground text-sm">
          Installs Docker (if missing) and runs the Basse agent over SSH. Safe to run again.
        </p>
        {data.status === "active" ? (
          <p className="mt-3 text-success-foreground text-sm">
            Agent active
            {data.lastSeenAt
              ? ` · last seen ${new Date(data.lastSeenAt).toLocaleString()}`
              : null}
          </p>
        ) : null}
        <div className="mt-4 flex items-center gap-2">
          <Button
            loading={provision.isPending || data.status === "provisioning"}
            onClick={() => provision.mutate()}
            disabled={data.status === "provisioning"}
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

      <DomainsSection serverId={serverId} sshHost={data.sshHost} />

      <div className="max-w-2xl rounded-lg border border-destructive/24 bg-card p-6">
        <h2 className="text-lg font-semibold">Danger zone</h2>
        <p className="mt-1 text-muted-foreground text-sm">
          Deleting a server removes it from Basse and discards its access key. Any running agent
          container is left in place.
        </p>
        <Button
          className="mt-4"
          loading={remove.isPending}
          onClick={() => remove.mutate()}
          variant="destructive"
        >
          Delete server
        </Button>
      </div>
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
    // Poll while any domain is mid-sync.
    refetchInterval: (query) =>
      (query.state.data ?? []).some((d) => d.status === "pending") ? 2000 : false,
  });

  const add = useMutation({
    mutationFn: () => createDomain(serverId, { host, upstream }),
    onSuccess: async () => {
      setHost("");
      setUpstream("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteDomain(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  const resync = useMutation({
    mutationFn: () => resyncProxy(serverId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  const domainList = domains.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <div className="flex items-center justify-between gap-3">
        <h2 className="text-lg font-semibold">Domains</h2>
        <Button loading={resync.isPending} onClick={() => resync.mutate()} size="sm" variant="outline">
          Resync proxy
        </Button>
      </div>
      <p className="mt-1 text-muted-foreground text-sm">
        Point each domain's DNS A record at <code className="font-mono">{sshHost}</code>, then it is
        routed to its upstream with automatic HTTPS.
      </p>

      <div className="mt-5">
        {domains.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : domainList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No domains yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {domainList.map((d) => (
              <li
                key={d.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
              >
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <p className="truncate font-medium text-sm">{d.host}</p>
                    <Badge variant={DOMAIN_STATUS_VARIANT[d.status]}>{d.status}</Badge>
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

      <form className="mt-6 space-y-4 border-t pt-6" onSubmit={handleSubmit}>
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

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button loading={add.isPending} type="submit">
          Add domain
        </Button>
      </form>
    </div>
  );
}
