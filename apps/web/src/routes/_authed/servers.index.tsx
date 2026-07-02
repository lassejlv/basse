import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { CheckIcon, CopyIcon, PlusIcon, ServerIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
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
import { Textarea } from "@/components/ui/textarea";
import { authClient } from "@/lib/auth-client";
import { maskHost, relativeTime } from "@/lib/format";
import { createServer, listServers } from "@/lib/servers";
import { listSshKeys } from "@/lib/ssh-keys";
import { toast } from "@/lib/toast";
import { cn } from "@/lib/utils";

type KeySource = "generate" | "saved" | "paste";
type ConnectionMode = "ssh" | "outbound";

export const Route = createFileRoute("/_authed/servers/")({
  component: ServersRoute,
});

function ServersRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;

  const servers = useQuery({
    queryKey: ["servers", organizationId],
    queryFn: listServers,
    enabled: Boolean(organizationId),
    refetchInterval: 30_000,
  });

  const serverList = servers.data ?? [];

  return (
    <section className="flex flex-1 flex-col gap-7 p-4 md:p-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
            Infrastructure
          </p>
          <h1 className="mt-1 font-semibold text-2xl tracking-tight md:text-3xl">Servers</h1>
          <p className="mt-1 text-muted-foreground text-sm">
            Connect machines; Basse provisions Docker and the agent, then deploys onto them.
          </p>
        </div>
        <AddServerDialog organizationId={organizationId} />
      </div>

      {servers.isPending ? (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          <div className="h-28 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
          <div className="h-28 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
          <div className="h-28 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
        </div>
      ) : serverList.length === 0 ? (
        <Empty className="rounded-2xl border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <ServerIcon />
            </EmptyMedia>
            <EmptyTitle>No servers connected</EmptyTitle>
            <EmptyDescription>
              Connect over SSH, or run the outbound agent when SSH is not possible.
            </EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
            <AddServerDialog organizationId={organizationId} />
          </EmptyContent>
        </Empty>
      ) : (
        <div className="grid gap-3 sm:grid-cols-2 xl:grid-cols-3">
          {serverList.map((server) => (
            <Link
              className="group rounded-xl border bg-card p-4 shadow-sm transition-[border-color] hover:border-muted-foreground/40"
              key={server.id}
              params={{ serverId: server.id }}
              to="/servers/$serverId"
            >
              <div className="flex items-center justify-between gap-2">
                <p className="min-w-0 truncate font-medium">{server.name}</p>
                <ServerStatusBadge status={server.status} />
              </div>
              <p className="mt-1 truncate font-mono text-muted-foreground text-xs">
                {server.connectionMode === "outbound"
                  ? maskHost(server.sshHost)
                  : `${server.sshUser}@${maskHost(server.sshHost)}:${server.sshPort}`}
              </p>
              <div className="mt-3 flex items-center gap-2">
                <Badge size="sm" variant="outline">
                  {server.isSystem
                    ? "local"
                    : server.connectionMode === "outbound"
                      ? "outbound agent"
                      : "ssh"}
                </Badge>
                <span className="ml-auto text-muted-foreground text-xs">
                  {server.lastSeenAt
                    ? `seen ${relativeTime(server.lastSeenAt)}`
                    : `added ${relativeTime(server.createdAt)}`}
                </span>
              </div>
              {server.statusMessage ? (
                <p
                  className={cn(
                    "mt-2 truncate text-xs",
                    server.status === "error" || server.status === "unreachable"
                      ? "text-destructive-foreground"
                      : "text-muted-foreground",
                  )}
                  title={server.statusMessage}
                >
                  {server.statusMessage}
                </p>
              ) : null}
            </Link>
          ))}
        </div>
      )}
    </section>
  );
}

function ChoiceCard({
  checked,
  title,
  description,
  onSelect,
  disabled,
  name,
}: {
  checked: boolean;
  title: string;
  description: string;
  onSelect: () => void;
  disabled?: boolean;
  name: string;
}) {
  return (
    <label
      className={cn(
        "flex cursor-pointer items-start gap-2.5 rounded-lg border px-3 py-2.5 text-sm transition",
        checked ? "border-primary/50 bg-primary/5" : "hover:bg-accent/40",
        disabled && "cursor-not-allowed opacity-60",
      )}
    >
      <input
        checked={checked}
        className="sr-only"
        disabled={disabled}
        name={name}
        onChange={onSelect}
        type="radio"
      />
      <span
        aria-hidden
        className={cn(
          "mt-0.5 flex size-4 shrink-0 items-center justify-center rounded-full border",
          checked && "border-primary bg-primary",
        )}
      >
        {checked ? <span className="size-1.5 rounded-full bg-primary-foreground" /> : null}
      </span>
      <span className="min-w-0">
        <span className="block font-medium">{title}</span>
        <span className="block text-muted-foreground text-xs">{description}</span>
      </span>
    </label>
  );
}

function AddServerDialog({ organizationId }: { organizationId: string | undefined }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const [name, setName] = useState("");
  const [sshHost, setSshHost] = useState("");
  const [sshUser, setSshUser] = useState("root");
  const [sshPort, setSshPort] = useState("22");
  const [connectionMode, setConnectionMode] = useState<ConnectionMode>("ssh");
  const [keySource, setKeySource] = useState<KeySource>("generate");
  const [sshKeyId, setSshKeyId] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [installCommand, setInstallCommand] = useState("");
  const [copied, setCopied] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const sshKeys = useQuery({
    queryKey: ["ssh-keys", organizationId],
    queryFn: listSshKeys,
    enabled: Boolean(organizationId) && open,
  });
  const storedKeys = (sshKeys.data ?? []).filter((key) => key.hasPrivateKey);

  function reset() {
    setName("");
    setSshHost("");
    setSshUser("root");
    setSshPort("22");
    setConnectionMode("ssh");
    setKeySource("generate");
    setSshKeyId("");
    setPrivateKey("");
    setInstallCommand("");
    setCopied(false);
    setError(null);
  }

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
      setError(null);
      await queryClient.invalidateQueries({ queryKey: ["servers", organizationId] });
      toast.success("Server added");
      if (created.agentInstallCommand) {
        // Outbound servers need the install command run on the machine —
        // keep the dialog open so it can be copied.
        setInstallCommand(created.agentInstallCommand);
      } else {
        reset();
        setOpen(false);
      }
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
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
          <Button>
            <PlusIcon />
            Add server
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-lg">
        {installCommand ? (
          <>
            <DialogHeader>
              <DialogTitle>Run the agent installer</DialogTitle>
              <DialogDescription>
                Run this on the server. The agent connects out to Basse — no SSH needed.
              </DialogDescription>
            </DialogHeader>
            <DialogPanel className="space-y-3">
              <Textarea
                className="min-h-40 font-mono text-xs"
                readOnly
                spellCheck={false}
                value={installCommand}
              />
              <Button
                onClick={() => {
                  void navigator.clipboard.writeText(installCommand);
                  setCopied(true);
                  toast.success("Install command copied");
                }}
                type="button"
                variant="outline"
              >
                {copied ? <CheckIcon /> : <CopyIcon />}
                Copy command
              </Button>
              <p className="text-muted-foreground text-xs">
                This command is shown once. The server appears as active when the agent connects.
              </p>
            </DialogPanel>
            <DialogFooter>
              <DialogClose render={<Button>Done</Button>} />
            </DialogFooter>
          </>
        ) : (
          <form onSubmit={handleSubmit}>
            <DialogHeader>
              <DialogTitle>Add a server</DialogTitle>
              <DialogDescription>
                Basse provisions Docker and the agent, then deploys apps onto it.
              </DialogDescription>
            </DialogHeader>
            <DialogPanel className="space-y-4">
              <fieldset className="space-y-2">
                <Label>Connection</Label>
                <div className="grid gap-2">
                  <ChoiceCard
                    checked={connectionMode === "ssh"}
                    description="Basse connects to the server over SSH and provisions the agent."
                    name="connection-mode"
                    onSelect={() => setConnectionMode("ssh")}
                    title="SSH (recommended)"
                  />
                  <ChoiceCard
                    checked={connectionMode === "outbound"}
                    description="Run a Docker command on the server when SSH is not possible."
                    name="connection-mode"
                    onSelect={() => setConnectionMode("outbound")}
                    title="Outbound agent (experimental)"
                  />
                </div>
              </fieldset>
              <div className="space-y-2">
                <Label htmlFor="server-name">Name</Label>
                <Input
                  id="server-name"
                  onChange={(event) => setName(event.currentTarget.value)}
                  placeholder="production-1"
                  required
                  value={name}
                />
              </div>
              <div className="flex gap-3">
                <div className="flex-1 space-y-2">
                  <Label htmlFor="server-host">
                    {connectionMode === "outbound" ? "Server address" : "SSH host"}
                  </Label>
                  <Input
                    id="server-host"
                    onChange={(event) => setSshHost(event.currentTarget.value)}
                    placeholder="203.0.113.10"
                    required
                    value={sshHost}
                  />
                </div>
                {connectionMode === "ssh" ? (
                  <div className="w-28 space-y-2">
                    <Label htmlFor="server-port">Port</Label>
                    <Input
                      id="server-port"
                      onChange={(event) => setSshPort(event.currentTarget.value)}
                      required
                      type="number"
                      value={sshPort}
                    />
                  </div>
                ) : null}
              </div>
              {connectionMode === "ssh" ? (
                <div className="space-y-2">
                  <Label htmlFor="server-user">SSH user</Label>
                  <Input
                    id="server-user"
                    onChange={(event) => setSshUser(event.currentTarget.value)}
                    placeholder="root"
                    required
                    value={sshUser}
                  />
                </div>
              ) : null}

              {connectionMode === "ssh" ? (
                <fieldset className="space-y-2">
                  <Label>SSH key</Label>
                  <div className="grid gap-2">
                    <ChoiceCard
                      checked={keySource === "generate"}
                      description="Basse creates a keypair; you add the public key to the server."
                      name="key-source"
                      onSelect={() => setKeySource("generate")}
                      title="Generate a new key"
                    />
                    <ChoiceCard
                      checked={keySource === "saved"}
                      description="Select a private key saved in Secrets."
                      disabled={storedKeys.length === 0}
                      name="key-source"
                      onSelect={() => {
                        setKeySource("saved");
                        setSshKeyId((current) => current || storedKeys[0]?.id || "");
                      }}
                      title="Use a saved key"
                    />
                    <ChoiceCard
                      checked={keySource === "paste"}
                      description="Paste a private key already trusted on the server."
                      name="key-source"
                      onSelect={() => setKeySource("paste")}
                      title="Use my own key"
                    />
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

              {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
            </DialogPanel>
            <DialogFooter>
              <DialogClose render={<Button variant="outline">Cancel</Button>} />
              <Button
                disabled={
                  !organizationId ||
                  (connectionMode === "ssh" && keySource === "saved" && !sshKeyId)
                }
                loading={add.isPending}
                type="submit"
              >
                Add server
              </Button>
            </DialogFooter>
          </form>
        )}
      </DialogPopup>
    </Dialog>
  );
}
