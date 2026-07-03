import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { CheckIcon, CopyIcon, RefreshCwIcon, TrashIcon } from "lucide-react";
import { FormEvent, useEffect, useState } from "react";
import type { LoadBalancerProvider, WorkspaceRole } from "@basse/shared";
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
import { authClient } from "@/lib/auth-client";
import {
  deleteLoadBalancerIntegration,
  listLoadBalancerIntegrations,
  saveLoadBalancerIntegration,
} from "@/lib/load-balancers";
import { checkSystemUpdate, getSystemInfo } from "@/lib/system";
import { toast } from "@/lib/toast";
import {
  deleteTeamInvitation,
  deleteTeamMember,
  getTeam,
  inviteTeamMember,
  updateTeamMember,
} from "@/lib/team";
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

      <TeamCard />

      <SelfHostedUpdatesCard />

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

function shortSha(value: string | null | undefined): string {
  if (!value || value === "unknown") return "unknown";
  return value.slice(0, 12);
}

function SelfHostedUpdatesCard() {
  const system = useQuery({
    queryKey: ["system-info"],
    queryFn: getSystemInfo,
  });
  const [copied, setCopied] = useState(false);
  const check = useMutation({
    mutationFn: checkSystemUpdate,
    onSuccess: (result) => {
      if (!result.selfHosted) {
        toast.info("Cloud-managed instance", { description: result.message });
      } else if (result.updateAvailable === true) {
        toast.success("Update available", { description: result.message });
      } else if (result.updateAvailable === false) {
        toast.success("Basse is up to date", { description: result.message });
      } else {
        toast.info("Update status unknown", { description: result.message });
      }
    },
    onError: (error: Error) =>
      toast.error("Couldn't check updates", { description: error.message }),
  });
  const latest = check.data;
  const command =
    latest?.updateCommand ?? system.data?.updateCommand ?? "cd /data/basse && ./update.sh";
  const selfHosted = latest?.selfHosted ?? system.data?.selfHosted ?? true;

  async function copyCommand() {
    await navigator.clipboard.writeText(command);
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1500);
    toast.success("Update command copied");
  }

  // Cloud instances are updated by the deploy pipeline — nothing to manage here.
  if (system.data && !system.data.selfHosted) return null;

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="text-lg font-semibold">Updates</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Check the latest self-hosted release and copy the host update command.
          </p>
        </div>
        <Button
          disabled={system.isPending}
          loading={check.isPending}
          onClick={() => check.mutate()}
          variant="outline"
        >
          <RefreshCwIcon />
          Check updates
        </Button>
      </div>

      {system.isError ? (
        <p className="mt-4 text-destructive-foreground text-sm">{system.error.message}</p>
      ) : null}

      <dl className="mt-4 grid grid-cols-[8rem_1fr] gap-y-3 text-sm">
        <dt className="text-muted-foreground">Mode</dt>
        <dd className="font-medium">{selfHosted ? "Self-hosted" : "Cloud"}</dd>
        <dt className="text-muted-foreground">Current</dt>
        <dd className="font-mono text-xs">
          {shortSha(latest?.currentCommitSha ?? system.data?.currentCommitSha)}
        </dd>
        <dt className="text-muted-foreground">Latest</dt>
        <dd className="font-mono text-xs">{shortSha(latest?.latestCommitSha)}</dd>
        <dt className="text-muted-foreground">Status</dt>
        <dd>
          {latest
            ? latest.message
            : selfHosted
              ? "Run a check to compare this instance with GitHub main."
              : "Cloud instances are updated by the deploy pipeline."}
        </dd>
      </dl>

      {selfHosted ? (
        <div className="mt-4 flex items-center gap-2 rounded-md border bg-muted/20 p-2">
          <code className="min-w-0 flex-1 truncate font-mono text-xs">{command}</code>
          <Button
            aria-label="Copy update command"
            onClick={copyCommand}
            size="icon"
            variant="outline"
          >
            {copied ? <CheckIcon /> : <CopyIcon />}
          </Button>
        </div>
      ) : null}
    </div>
  );
}

function TeamCard() {
  const queryClient = useQueryClient();
  const queryKey = ["team"];
  const team = useQuery({ queryKey, queryFn: getTeam });
  const [email, setEmail] = useState("");
  const [role, setRole] = useState<WorkspaceRole>("member");

  const invalidate = () => queryClient.invalidateQueries({ queryKey });

  const invite = useMutation({
    mutationFn: () => inviteTeamMember({ email, role }),
    onSuccess: async () => {
      setEmail("");
      toast.success("Team updated");
      await invalidate();
    },
    onError: (error: Error) =>
      toast.error("Couldn't invite member", { description: error.message }),
  });

  const updateRole = useMutation({
    mutationFn: (input: { id: string; role: WorkspaceRole }) =>
      updateTeamMember(input.id, { role: input.role }),
    onSuccess: async () => {
      toast.success("Role updated");
      await invalidate();
    },
    onError: (error: Error) => toast.error("Couldn't update role", { description: error.message }),
  });

  const removeMember = useMutation({
    mutationFn: deleteTeamMember,
    onSuccess: async () => {
      toast.success("Member removed");
      await invalidate();
    },
    onError: (error: Error) =>
      toast.error("Couldn't remove member", { description: error.message }),
  });

  const cancelInvite = useMutation({
    mutationFn: deleteTeamInvitation,
    onSuccess: async () => {
      toast.success("Invitation cancelled");
      await invalidate();
    },
    onError: (error: Error) =>
      toast.error("Couldn't cancel invitation", { description: error.message }),
  });

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Team</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Invite teammates and control workspace access.
      </p>

      <form
        className="mt-4 grid gap-3 sm:grid-cols-[1fr_140px_auto]"
        onSubmit={(event) => {
          event.preventDefault();
          invite.mutate();
        }}
      >
        <Input
          aria-label="Invite email"
          onChange={(event) => setEmail(event.currentTarget.value)}
          placeholder="teammate@example.com"
          type="email"
          value={email}
        />
        <RoleSelect onChange={setRole} value={role} />
        <Button disabled={!email.trim()} loading={invite.isPending} type="submit">
          Invite
        </Button>
      </form>

      <div className="mt-5 space-y-2">
        {team.isPending ? (
          <p className="text-muted-foreground text-sm">Loading team…</p>
        ) : team.isError ? (
          <p className="text-destructive-foreground text-sm">{team.error.message}</p>
        ) : (
          <>
            {team.data.members.map((member) => (
              <div
                className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
                key={member.id}
              >
                <div className="min-w-0">
                  <p className="truncate font-medium text-sm">{member.name || member.email}</p>
                  <p className="truncate text-muted-foreground text-xs">{member.email}</p>
                </div>
                <div className="flex items-center gap-2">
                  <RoleSelect
                    onChange={(nextRole) => updateRole.mutate({ id: member.id, role: nextRole })}
                    value={member.role}
                  />
                  <Button
                    aria-label={`Remove ${member.email}`}
                    loading={removeMember.isPending && removeMember.variables === member.id}
                    onClick={() => removeMember.mutate(member.id)}
                    size="icon"
                    variant="outline"
                  >
                    <TrashIcon />
                  </Button>
                </div>
              </div>
            ))}
            {team.data.invitations.map((invitation) => (
              <div
                className="flex items-center justify-between gap-3 rounded-md border border-dashed px-3 py-2"
                key={invitation.id}
              >
                <div className="min-w-0">
                  <p className="truncate font-medium text-sm">{invitation.email}</p>
                  <p className="truncate text-muted-foreground text-xs">
                    Pending · {invitation.role}
                  </p>
                </div>
                <Button
                  loading={cancelInvite.isPending && cancelInvite.variables === invitation.id}
                  onClick={() => cancelInvite.mutate(invitation.id)}
                  size="sm"
                  variant="outline"
                >
                  Cancel
                </Button>
              </div>
            ))}
          </>
        )}
      </div>
    </div>
  );
}

function RoleSelect({
  value,
  onChange,
}: {
  value: WorkspaceRole;
  onChange: (value: WorkspaceRole) => void;
}) {
  return (
    <Select value={value} onValueChange={(next) => onChange((next ?? "member") as WorkspaceRole)}>
      <SelectTrigger className="w-full">
        <SelectValue placeholder="Role">{(next: WorkspaceRole) => next}</SelectValue>
      </SelectTrigger>
      <SelectPopup>
        <SelectItem value="owner">Owner</SelectItem>
        <SelectItem value="admin">Admin</SelectItem>
        <SelectItem value="member">Member</SelectItem>
      </SelectPopup>
    </Select>
  );
}

function LoadBalancerIntegrationsCard() {
  const queryClient = useQueryClient();
  const queryKey = ["load-balancer-integrations"];
  const integrations = useQuery({
    queryKey,
    queryFn: listLoadBalancerIntegrations,
  });
  const [provider, setProvider] = useState<LoadBalancerProvider>("hetzner");
  const [name, setName] = useState("Hetzner");
  const [token, setToken] = useState("");
  const [error, setError] = useState<string | null>(null);
  const providerLabel = trafficProviderLabel(provider);

  useEffect(() => {
    setName(providerLabel);
  }, [providerLabel]);

  const save = useMutation({
    mutationFn: () => saveLoadBalancerIntegration({ provider, name, token }),
    onSuccess: async () => {
      setToken("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
      toast.success(`${providerLabel} connected`);
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
        <div className="grid gap-3 sm:grid-cols-[150px_150px_1fr]">
          <div className="space-y-2">
            <Label>Provider</Label>
            <Select
              value={provider}
              onValueChange={(value) => setProvider((value ?? "hetzner") as LoadBalancerProvider)}
            >
              <SelectTrigger className="w-full">
                <SelectValue placeholder="Provider">
                  {(value: LoadBalancerProvider) => trafficProviderLabel(value)}
                </SelectValue>
              </SelectTrigger>
              <SelectPopup>
                <SelectItem value="hetzner">Hetzner</SelectItem>
                <SelectItem value="cloudflare">Cloudflare</SelectItem>
              </SelectPopup>
            </Select>
          </div>
          <div className="space-y-2">
            <Label htmlFor="traffic-provider-name">Name</Label>
            <Input
              id="traffic-provider-name"
              onChange={(event) => setName(event.currentTarget.value)}
              value={name}
            />
          </div>
          <div className="space-y-2">
            <Label htmlFor="traffic-provider-token">{providerLabel} API token</Label>
            <Input
              id="traffic-provider-token"
              onChange={(event) => setToken(event.currentTarget.value)}
              placeholder={provider === "hetzner" ? "hcloud token" : "Cloudflare API token"}
              type="password"
              value={token}
            />
          </div>
        </div>
        <p className="text-muted-foreground text-xs">
          {provider === "hetzner"
            ? "The token is validated against Hetzner Cloud and stored encrypted."
            : "The token is validated against Cloudflare and needs Zone Read, Zone Load Balancers Edit, and Account Load Balancing Monitors and Pools Edit."}
        </p>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button disabled={!token.trim()} loading={save.isPending} type="submit">
          Connect {providerLabel}
        </Button>
      </form>
    </div>
  );
}

function trafficProviderLabel(provider: LoadBalancerProvider): string {
  return provider === "hetzner" ? "Hetzner" : "Cloudflare";
}
