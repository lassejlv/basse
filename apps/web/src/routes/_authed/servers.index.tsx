import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { FormEvent, useState } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Textarea } from "@/components/ui/textarea";
import { authClient } from "@/lib/auth-client";
import { createServer, listServers } from "@/lib/servers";
import { listSshKeys } from "@/lib/ssh-keys";
import { toast } from "@/lib/toast";

type KeySource = "generate" | "saved" | "paste";
type ConnectionMode = "ssh" | "outbound";

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
  const [connectionMode, setConnectionMode] = useState<ConnectionMode>("ssh");
  const [keySource, setKeySource] = useState<KeySource>("generate");
  const [sshKeyId, setSshKeyId] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [installCommand, setInstallCommand] = useState("");
  const [error, setError] = useState<string | null>(null);

  const servers = useQuery({
    queryKey,
    queryFn: listServers,
    enabled: Boolean(organizationId),
  });

  const sshKeys = useQuery({
    queryKey: ["ssh-keys", organizationId],
    queryFn: listSshKeys,
    enabled: Boolean(organizationId),
  });

  const add = useMutation({
    mutationFn: () =>
      createServer({
        name,
        sshHost,
        connectionMode,
        sshUser: connectionMode === "ssh" ? sshUser : undefined,
        sshPort: connectionMode === "ssh" ? Number(sshPort) : undefined,
        sshKeyId: connectionMode === "ssh" && keySource === "saved" ? sshKeyId : undefined,
        privateKey: connectionMode === "ssh" && keySource === "paste" ? privateKey : undefined,
      }),
    onSuccess: async (created) => {
      setName("");
      setSshHost("");
      setSshUser("root");
      setSshPort("22");
      setConnectionMode("ssh");
      setKeySource("generate");
      setSshKeyId("");
      setPrivateKey("");
      setInstallCommand(created.agentInstallCommand ?? "");
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
  const storedKeys = (sshKeys.data ?? []).filter((key) => key.hasPrivateKey);

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
                      {srv.connectionMode === "outbound"
                        ? `outbound · ${srv.sshHost}`
                        : `${srv.sshUser}@${srv.sshHost}:${srv.sshPort}`}
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
        <fieldset className="space-y-2">
          <Label>Connection</Label>
          <div className="flex flex-col gap-2">
            <label className="flex items-start gap-2 text-sm">
              <input
                checked={connectionMode === "ssh"}
                className="mt-0.5"
                name="connection-mode"
                onChange={() => setConnectionMode("ssh")}
                type="radio"
              />
              <span>
                <span className="font-medium">SSH</span>
                <span className="block text-muted-foreground text-xs">
                  Basse connects to the server over SSH and provisions the agent.
                </span>
              </span>
            </label>
            <label className="flex items-start gap-2 text-sm">
              <input
                checked={connectionMode === "outbound"}
                className="mt-0.5"
                name="connection-mode"
                onChange={() => setConnectionMode("outbound")}
                type="radio"
              />
              <span>
                <span className="font-medium">Outbound agent</span>
                <span className="block text-muted-foreground text-xs">
                  Run a Docker command on the server; no inbound SSH access is required.
                </span>
              </span>
            </label>
          </div>
        </fieldset>
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
            <Label htmlFor="server-host">
              {connectionMode === "outbound" ? "Server address" : "SSH host"}
            </Label>
            <Input
              id="server-host"
              value={sshHost}
              onChange={(event) => setSshHost(event.currentTarget.value)}
              placeholder="203.0.113.10"
              required
            />
          </div>
          {connectionMode === "ssh" ? (
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
          ) : null}
        </div>
        {connectionMode === "ssh" ? (
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
        ) : null}

        {connectionMode === "ssh" ? (
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
                  checked={keySource === "saved"}
                  className="mt-0.5"
                  disabled={storedKeys.length === 0}
                  name="key-source"
                  onChange={() => {
                    setKeySource("saved");
                    setSshKeyId((current) => current || storedKeys[0]?.id || "");
                  }}
                  type="radio"
                />
                <span>
                  <span className="font-medium">Use a saved key</span>
                  <span className="block text-muted-foreground text-xs">
                    Select a private key saved in Secrets.
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
            {keySource === "saved" ? (
              <div className="space-y-2">
                <Select value={sshKeyId} onValueChange={(value) => setSshKeyId(value ?? "")}>
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="Select SSH key">
                      {(value: string) =>
                        storedKeys.find((key) => key.id === value)?.name ?? "Select SSH key"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    {storedKeys.map((key) => (
                      <SelectItem key={key.id} value={key.id}>
                        {key.name}
                      </SelectItem>
                    ))}
                  </SelectPopup>
                </Select>
                {storedKeys.length === 0 ? (
                  <p className="text-muted-foreground text-xs">
                    Add a private SSH key in Secrets before using this option.
                  </p>
                ) : null}
              </div>
            ) : null}
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
        ) : null}

        {installCommand ? (
          <div className="space-y-2 rounded-md border bg-muted/30 p-3">
            <Label htmlFor="outbound-install">Outbound install command</Label>
            <Textarea
              className="font-mono text-xs"
              id="outbound-install"
              readOnly
              rows={8}
              value={installCommand}
            />
            <Button
              onClick={() => {
                void navigator.clipboard.writeText(installCommand);
                toast.success("Install command copied");
              }}
              type="button"
              variant="secondary"
            >
              Copy command
            </Button>
          </div>
        ) : null}

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button
          disabled={
            !organizationId || (connectionMode === "ssh" && keySource === "saved" && !sshKeyId)
          }
          loading={add.isPending}
          type="submit"
        >
          Add server
        </Button>
      </form>
    </section>
  );
}
