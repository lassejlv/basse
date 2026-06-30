import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import { ArrowLeftIcon } from "lucide-react";
import { useState } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Button } from "@/components/ui/button";
import { authClient } from "@/lib/auth-client";
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
