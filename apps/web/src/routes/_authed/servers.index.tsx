import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { FormEvent, useState } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { authClient } from "@/lib/auth-client";
import { createServer, listServers } from "@/lib/servers";
import { toast } from "@/lib/toast";

type KeySource = "generate" | "paste";

export const Route = createFileRoute("/_authed/servers/")({
  component: ServersRoute,
});

function ServersRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;
  const queryClient = useQueryClient();
  const queryKey = ["servers", organizationId];

  const [name, setName] = useState("");
  const [sshHost, setSshHost] = useState("");
  const [sshUser, setSshUser] = useState("root");
  const [sshPort, setSshPort] = useState("22");
  const [keySource, setKeySource] = useState<KeySource>("generate");
  const [privateKey, setPrivateKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const servers = useQuery({
    queryKey,
    queryFn: listServers,
    enabled: Boolean(organizationId),
  });

  const add = useMutation({
    mutationFn: () =>
      createServer({
        name,
        sshHost,
        sshUser,
        sshPort: Number(sshPort),
        privateKey: keySource === "paste" ? privateKey : undefined,
      }),
    onSuccess: async () => {
      setName("");
      setSshHost("");
      setSshUser("root");
      setSshPort("22");
      setKeySource("generate");
      setPrivateKey("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Server added");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  const serverList = servers.data ?? [];

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Servers</h1>
        <p className="mt-2 text-muted-foreground text-sm">
          Connect a server, then provision Docker and the Basse agent over SSH.
        </p>
      </div>

      <div className="max-w-2xl">
        {servers.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : serverList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No servers yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {serverList.map((srv) => (
              <li key={srv.id}>
                <Link
                  to="/servers/$serverId"
                  params={{ serverId: srv.id }}
                  className="flex items-center justify-between gap-3 rounded-lg border bg-card px-4 py-3 hover:bg-accent/40"
                >
                  <div className="min-w-0">
                    <p className="truncate font-medium">{srv.name}</p>
                    <p className="truncate font-mono text-muted-foreground text-xs">
                      {srv.sshUser}@{srv.sshHost}:{srv.sshPort}
                    </p>
                  </div>
                  <ServerStatusBadge status={srv.status} />
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>

      <form className="max-w-2xl space-y-4 rounded-lg border bg-card p-6" onSubmit={handleSubmit}>
        <h2 className="text-lg font-semibold">Add a server</h2>
        <div className="space-y-2">
          <Label htmlFor="server-name">Name</Label>
          <Input
            id="server-name"
            value={name}
            onChange={(event) => setName(event.currentTarget.value)}
            placeholder="production-1"
            required
          />
        </div>
        <div className="flex gap-3">
          <div className="flex-1 space-y-2">
            <Label htmlFor="server-host">SSH host</Label>
            <Input
              id="server-host"
              value={sshHost}
              onChange={(event) => setSshHost(event.currentTarget.value)}
              placeholder="203.0.113.10"
              required
            />
          </div>
          <div className="w-28 space-y-2">
            <Label htmlFor="server-port">Port</Label>
            <Input
              id="server-port"
              type="number"
              value={sshPort}
              onChange={(event) => setSshPort(event.currentTarget.value)}
              required
            />
          </div>
        </div>
        <div className="space-y-2">
          <Label htmlFor="server-user">SSH user</Label>
          <Input
            id="server-user"
            value={sshUser}
            onChange={(event) => setSshUser(event.currentTarget.value)}
            placeholder="root"
            required
          />
        </div>

        <fieldset className="space-y-2">
          <Label>SSH key</Label>
          <div className="flex flex-col gap-2">
            <label className="flex items-start gap-2 text-sm">
              <input
                checked={keySource === "generate"}
                className="mt-0.5"
                name="key-source"
                onChange={() => setKeySource("generate")}
                type="radio"
              />
              <span>
                <span className="font-medium">Generate a new key</span>
                <span className="block text-muted-foreground text-xs">
                  Basse creates a keypair; you add the public key to the server.
                </span>
              </span>
            </label>
            <label className="flex items-start gap-2 text-sm">
              <input
                checked={keySource === "paste"}
                className="mt-0.5"
                name="key-source"
                onChange={() => setKeySource("paste")}
                type="radio"
              />
              <span>
                <span className="font-medium">Use my own key</span>
                <span className="block text-muted-foreground text-xs">
                  Paste a private key already trusted on the server.
                </span>
              </span>
            </label>
          </div>
          {keySource === "paste" ? (
            <Textarea
              aria-label="Private key"
              className="font-mono text-xs"
              onChange={(event) => setPrivateKey(event.currentTarget.value)}
              placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
              required
              rows={5}
              value={privateKey}
            />
          ) : null}
        </fieldset>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button disabled={!organizationId} loading={add.isPending} type="submit">
          Add server
        </Button>
      </form>
    </section>
  );
}
