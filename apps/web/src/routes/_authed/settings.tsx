import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { TrashIcon } from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
import {
  deleteLoadBalancerIntegration,
  listLoadBalancerIntegrations,
  saveLoadBalancerIntegration,
} from "@/lib/load-balancers";
import { toast } from "@/lib/toast";
import { getWorkspaceSettings, updateWorkspaceSettings } from "@/lib/workspace-settings";

export const Route = createFileRoute("/_authed/settings")({
  component: SettingsRoute,
});

function SettingsRoute() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const { data: session } = authClient.useSession();
  const workspaceSettings = useQuery({
    queryKey: ["workspace-settings"],
    queryFn: getWorkspaceSettings,
  });
  const [imageRetentionDays, setImageRetentionDays] = useState("30");
  const [settingsError, setSettingsError] = useState<string | null>(null);

  useEffect(() => {
    if (workspaceSettings.data) {
      setImageRetentionDays(String(workspaceSettings.data.imageRetentionDays));
    }
  }, [workspaceSettings.data]);

  const signOut = useMutation({
    mutationFn: () => authClient.signOut(),
    onSuccess: () => navigate({ to: "/login" }),
  });

  const saveSettings = useMutation({
    mutationFn: () => updateWorkspaceSettings({ imageRetentionDays: Number(imageRetentionDays) }),
    onSuccess: async () => {
      setSettingsError(null);
      await queryClient.invalidateQueries({ queryKey: ["workspace-settings"] });
      toast.success("Image settings saved");
    },
    onError: (error: Error) => setSettingsError(error.message),
  });

  function submitSettings(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    saveSettings.mutate();
  }

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Settings</h1>
        <p className="mt-2 text-muted-foreground text-sm">Workspace and account settings.</p>
      </div>

      <div className="max-w-2xl rounded-lg border bg-card p-6">
        <h2 className="text-lg font-semibold">Workspace</h2>
        <dl className="mt-4 grid grid-cols-[8rem_1fr] gap-y-3 text-sm">
          <dt className="text-muted-foreground">Name</dt>
          <dd className="font-medium">{activeOrganization?.name ?? "—"}</dd>
          <dt className="text-muted-foreground">Slug</dt>
          <dd className="font-mono text-xs">{activeOrganization?.slug ?? "—"}</dd>
        </dl>
      </div>

      <div className="max-w-2xl rounded-lg border bg-card p-6">
        <h2 className="text-lg font-semibold">Images</h2>
        <p className="mt-1 text-muted-foreground text-sm">
          Control how long deployment images are kept before cleanup.
        </p>
        <form className="mt-4 space-y-4" onSubmit={submitSettings}>
          <div className="max-w-48 space-y-2">
            <Label htmlFor="image-retention-days">Retention days</Label>
            <Input
              id="image-retention-days"
              max={365}
              min={1}
              onChange={(event) => setImageRetentionDays(event.currentTarget.value)}
              type="number"
              value={imageRetentionDays}
            />
          </div>
          {workspaceSettings.isError ? (
            <p className="text-destructive-foreground text-sm">{workspaceSettings.error.message}</p>
          ) : null}
          {settingsError ? (
            <p className="text-destructive-foreground text-sm">{settingsError}</p>
          ) : null}
          <Button
            disabled={workspaceSettings.isPending}
            loading={saveSettings.isPending}
            type="submit"
          >
            Save image settings
          </Button>
        </form>
      </div>

      <LoadBalancerIntegrationsCard />

      <div className="max-w-2xl rounded-lg border bg-card p-6">
        <h2 className="text-lg font-semibold">Account</h2>
        <dl className="mt-4 grid grid-cols-[8rem_1fr] gap-y-3 text-sm">
          <dt className="text-muted-foreground">Name</dt>
          <dd className="font-medium">{session?.user.name || "—"}</dd>
          <dt className="text-muted-foreground">Email</dt>
          <dd>{session?.user.email ?? "—"}</dd>
        </dl>
        <Button
          className="mt-5"
          loading={signOut.isPending}
          onClick={() => signOut.mutate()}
          variant="outline"
        >
          Sign out
        </Button>
      </div>
    </section>
  );
}

function LoadBalancerIntegrationsCard() {
  const queryClient = useQueryClient();
  const queryKey = ["load-balancer-integrations"];
  const integrations = useQuery({
    queryKey,
    queryFn: listLoadBalancerIntegrations,
  });
  const [name, setName] = useState("Hetzner");
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);

  const save = useMutation({
    mutationFn: () => saveLoadBalancerIntegration({ provider: "hetzner", name, token }),
    onSuccess: async () => {
      setToken("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Hetzner connected");
    },
    onError: (saveError: Error) => setError(saveError.message),
  });

  const remove = useMutation({
    mutationFn: (id: string) => deleteLoadBalancerIntegration(id),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey });
      toast.success("Traffic provider removed");
    },
    onError: (removeError: Error) =>
      toast.error("Couldn't remove provider", { description: removeError.message }),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    save.mutate();
  }

  const list = integrations.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Traffic providers</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Connect provider APIs so Basse can create and sync managed load balancers for multi-server
        apps.
      </p>

      <div className="mt-5">
        {integrations.isPending ? (
          <p className="text-muted-foreground text-sm">Loading providers…</p>
        ) : list.length === 0 ? (
          <p className="text-muted-foreground text-sm">No traffic providers connected.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {list.map((integration) => (
              <li
                key={integration.id}
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
              >
                <div className="min-w-0">
                  <p className="truncate font-medium text-sm">{integration.name}</p>
                  <p className="truncate font-mono text-muted-foreground text-xs">
                    {integration.provider}
                    {integration.tokenHint ? ` · token ends ${integration.tokenHint}` : ""}
                    {integration.statusMessage ? ` · ${integration.statusMessage}` : ""}
                  </p>
                </div>
                <Button
                  aria-label={`Delete ${integration.name}`}
                  loading={remove.isPending && remove.variables === integration.id}
                  onClick={() => remove.mutate(integration.id)}
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
        <div className="grid gap-3 sm:grid-cols-[150px_1fr]">
          <div className="space-y-2">
            <Label htmlFor="traffic-provider-name">Name</Label>
            <Input
              id="traffic-provider-name"
              onChange={(event) => setName(event.currentTarget.value)}
              value={name}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="traffic-provider-token">Hetzner API token</Label>
            <Input
              id="traffic-provider-token"
              onChange={(event) => setToken(event.currentTarget.value)}
              placeholder="hcloud token"
              type="password"
              value={token}
            />
          </div>
        </div>
        <p className="text-muted-foreground text-xs">
          The token is validated against Hetzner Cloud and stored encrypted. Cloudflare will plug
          into the same provider model later.
        </p>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button disabled={!token.trim()} loading={save.isPending} type="submit">
          Connect Hetzner
        </Button>
      </form>
    </div>
  );
}
