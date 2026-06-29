import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { TrashIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { authClient } from "@/lib/auth-client";
import { disconnectDepot, getDepotConnection, saveDepotConnection } from "@/lib/depot";
import { createSshKey, deleteSshKey, listSshKeys } from "@/lib/ssh-keys";

export const Route = createFileRoute("/_authed/settings")({
  component: SettingsRoute,
});

function SettingsRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Settings</h1>
        <p className="mt-2 text-muted-foreground text-sm">
          Manage SSH keys and integrations for {activeOrganization?.name ?? "this workspace"}.
        </p>
      </div>

      <SshKeysSection organizationId={organizationId} />
      <DepotSection organizationId={organizationId} />
    </section>
  );
}

function SshKeysSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["ssh-keys", organizationId];
  const [name, setName] = useState("");
  const [publicKey, setPublicKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const keys = useQuery({
    queryKey,
    queryFn: listSshKeys,
    enabled: Boolean(organizationId),
  });

  const addKey = useMutation({
    mutationFn: () => createSshKey({ name, publicKey }),
    onSuccess: async () => {
      setName("");
      setPublicKey("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const removeKey = useMutation({
    mutationFn: (id: string) => deleteSshKey(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    addKey.mutate();
  }

  const keyList = keys.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">SSH Keys</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Public keys granted access to this workspace.
      </p>

      <div className="mt-5">
        {keys.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : keyList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No SSH keys yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {keyList.map((key) => (
              <li
                key={key.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate font-medium text-sm">{key.name}</p>
                  <p className="truncate font-mono text-muted-foreground text-xs">
                    {key.publicKey}
                  </p>
                </div>
                <Button
                  aria-label={`Delete ${key.name}`}
                  loading={removeKey.isPending && removeKey.variables === key.id}
                  onClick={() => removeKey.mutate(key.id)}
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
          <Label htmlFor="ssh-key-name">Name</Label>
          <Input
            id="ssh-key-name"
            value={name}
            onChange={(event) => setName(event.currentTarget.value)}
            placeholder="laptop"
            required
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="ssh-key-public">Public key</Label>
          <Textarea
            id="ssh-key-public"
            value={publicKey}
            onChange={(event) => setPublicKey(event.currentTarget.value)}
            placeholder="ssh-ed25519 AAAA… user@host"
            rows={3}
            required
          />
        </div>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button disabled={!organizationId} loading={addKey.isPending} type="submit">
          Add SSH key
        </Button>
      </form>
    </div>
  );
}

function DepotSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["depot-connection", organizationId];
  const [token, setToken] = useState("");
  const [projectId, setProjectId] = useState("");
  const [error, setError] = useState<string | null>(null);

  const connection = useQuery({
    queryKey,
    queryFn: getDepotConnection,
    enabled: Boolean(organizationId),
  });

  const save = useMutation({
    mutationFn: () => saveDepotConnection({ token, projectId }),
    onSuccess: async () => {
      setToken("");
      setProjectId("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const disconnect = useMutation({
    mutationFn: disconnectDepot,
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
    },
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    save.mutate();
  }

  const connected = connection.data?.connected;

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Depot</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Connect a Depot project to build images for this workspace.
      </p>

      {connected ? (
        <div className="mt-5 flex items-center justify-between gap-3 rounded-md border px-3 py-2">
          <div className="min-w-0 text-sm">
            <p className="font-medium">Connected</p>
            <p className="truncate font-mono text-muted-foreground text-xs">
              project {connection.data?.projectId} · token ••••{connection.data?.tokenHint}
            </p>
          </div>
          <Button
            loading={disconnect.isPending}
            onClick={() => disconnect.mutate()}
            variant="outline"
          >
            Disconnect
          </Button>
        </div>
      ) : null}

      <form className="mt-6 space-y-4 border-t pt-6" onSubmit={handleSubmit}>
        <div className="space-y-2">
          <Label htmlFor="depot-project">Project ID</Label>
          <Input
            id="depot-project"
            value={projectId}
            onChange={(event) => setProjectId(event.currentTarget.value)}
            placeholder="abc123def4"
            required
          />
        </div>
        <div className="space-y-2">
          <Label htmlFor="depot-token">Access token</Label>
          <Input
            id="depot-token"
            type="password"
            autoComplete="off"
            value={token}
            onChange={(event) => setToken(event.currentTarget.value)}
            placeholder="depot_org_…"
            required
          />
        </div>

        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}

        <Button disabled={!organizationId} loading={save.isPending} type="submit">
          {connected ? "Update connection" : "Connect Depot"}
        </Button>
      </form>
    </div>
  );
}
