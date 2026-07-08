import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  Building2Icon,
  CheckIcon,
  CopyIcon,
  LayersIcon,
  LogOutIcon,
  NetworkIcon,
  RefreshCwIcon,
  SlidersHorizontalIcon,
  TrashIcon,
  UsersIcon,
} from "lucide-react";
import { FormEvent, type ReactNode, useEffect, useState } from "react";
import type { LoadBalancerProvider, WorkspaceRole } from "@basse/shared";
import { EmptyNote, ErrorText, SectionLabel } from "@/components/dashboard";
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
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_authed/settings")({
  component: SettingsRoute,
});

/* Settings anatomy: a sticky in-page section nav on the left, a single column
   of labeled cards on the right, and definition rows for read-only facts. */

const SETTINGS_SECTIONS = [
  { id: "team", label: "Team" },
  { id: "updates", label: "Updates", selfHostedOnly: true },
  { id: "workspace", label: "Workspace" },
  { id: "images", label: "Images" },
  { id: "traffic", label: "Traffic" },
  { id: "account", label: "Account" },
] as const;

function SettingsRoute() {
  const navigate = useNavigate();
  const queryClient = useQueryClient();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const { data: session } = authClient.useSession();
  const system = useQuery({ queryKey: ["system-info"], queryFn: getSystemInfo });
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

  const selfHosted = system.data?.selfHosted ?? true;
  const sections = SETTINGS_SECTIONS.filter(
    (section) => !("selfHostedOnly" in section && section.selfHostedOnly && !selfHosted),
  );

  return (
    <section className="flex flex-1 flex-col gap-7 scroll-smooth p-4 md:p-6">
      <header>
        <SectionLabel>Workspace / Configuration</SectionLabel>
        <h1 className="mt-1 font-semibold text-2xl tracking-tight md:text-3xl">Settings</h1>
        <p className="mt-1 text-muted-foreground text-sm">
          Workspace and account settings for {activeOrganization?.name ?? "this workspace"}.
        </p>
      </header>

      <div className="grid max-w-4xl items-start gap-8 lg:grid-cols-[9rem_1fr]">
        <nav aria-label="Settings sections" className="sticky top-6 hidden lg:block">
          <ul className="flex flex-col gap-0.5 border-l">
            {sections.map((section) => (
              <li key={section.id}>
                <a
                  className="-ml-px block border-l border-transparent py-1.5 pl-3 font-mono text-muted-foreground text-xs uppercase tracking-[0.1em] transition-colors hover:border-foreground/40 hover:text-foreground"
                  href={`#${section.id}`}
                >
                  {section.label}
                </a>
              </li>
            ))}
          </ul>
        </nav>

        <div className="flex min-w-0 flex-col gap-4">
          <TeamCard />

          <SelfHostedUpdatesCard />

          <SettingsCard
            description="Identity of the active workspace."
            icon={<Building2Icon className="size-4" />}
            id="workspace"
            title="Workspace"
          >
            <dl className="mt-4 divide-y text-sm">
              <FactRow label="Name">{activeOrganization?.name ?? "—"}</FactRow>
              <FactRow label="Slug" mono>
                {activeOrganization?.slug ?? "—"}
              </FactRow>
            </dl>
          </SettingsCard>

          <SettingsCard
            description="Control how long deployment images are kept before cleanup."
            icon={<LayersIcon className="size-4" />}
            id="images"
            title="Images"
          >
            <form className="mt-4 space-y-4" onSubmit={submitSettings}>
              <div className="flex flex-wrap items-end gap-3">
                <div className="max-w-40 space-y-2">
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
                <Button
                  disabled={workspaceSettings.isPending}
                  loading={saveSettings.isPending}
                  size="sm"
                  type="submit"
                >
                  Save
                </Button>
              </div>
              <p className="text-muted-foreground text-xs">
                Older images are pruned from build storage automatically. 1–365 days.
              </p>
              {workspaceSettings.isError ? (
                <ErrorText>{workspaceSettings.error.message}</ErrorText>
              ) : null}
              {settingsError ? <ErrorText>{settingsError}</ErrorText> : null}
            </form>
          </SettingsCard>

          <LoadBalancerIntegrationsCard />

          <SettingsCard
            description="The account you are signed in with."
            icon={<SlidersHorizontalIcon className="size-4" />}
            id="account"
            title="Account"
          >
            <div className="mt-4 flex items-center gap-3">
              <Avatar name={session?.user.name || session?.user.email || "?"} />
              <div className="min-w-0">
                <p className="truncate font-medium text-sm">{session?.user.name || "—"}</p>
                <p className="truncate text-muted-foreground text-xs">
                  {session?.user.email ?? "—"}
                </p>
              </div>
            </div>
            <div className="mt-5 flex items-center justify-between gap-3 border-t pt-4">
              <p className="text-muted-foreground text-xs">
                Ends this session on this device only.
              </p>
              <Button
                loading={signOut.isPending}
                onClick={() => signOut.mutate()}
                size="sm"
                variant="outline"
              >
                <LogOutIcon />
                Sign out
              </Button>
            </div>
          </SettingsCard>
        </div>
      </div>
    </section>
  );
}

function SettingsCard({
  id,
  icon,
  title,
  description,
  action,
  children,
}: {
  id: string;
  icon: ReactNode;
  title: string;
  description: string;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card className="scroll-mt-6 p-5" id={id}>
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div className="flex items-start gap-3">
          <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-muted/30 text-foreground/80 shadow-[inset_0_1px_0_--theme(--color-white/6%)]">
            {icon}
          </span>
          <div className="min-w-0">
            <h2 className="font-semibold text-sm">{title}</h2>
            <p className="mt-0.5 text-muted-foreground text-xs leading-relaxed">{description}</p>
          </div>
        </div>
        {action}
      </div>
      {children}
    </Card>
  );
}

function FactRow({
  label,
  mono,
  children,
}: {
  label: string;
  mono?: boolean;
  children: ReactNode;
}) {
  return (
    <div className="grid grid-cols-[8rem_1fr] items-baseline gap-3 py-2.5 first:pt-0 last:pb-0">
      <dt className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.1em]">
        {label}
      </dt>
      <dd className={cn("min-w-0 truncate", mono ? "font-mono text-xs" : "font-medium text-sm")}>
        {children}
      </dd>
    </div>
  );
}

function Avatar({ name }: { name: string }) {
  const initials = name
    .split(/[\s@._-]+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase() ?? "")
    .join("");
  return (
    <span className="flex size-8 shrink-0 items-center justify-center rounded-full border bg-muted/40 font-medium font-mono text-[0.65rem] text-foreground/80">
      {initials || "?"}
    </span>
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
    <SettingsCard
      action={
        <div className="flex items-center gap-2">
          {latest?.updateAvailable === true ? (
            <Badge size="sm" variant="warning">
              Update available
            </Badge>
          ) : latest?.updateAvailable === false ? (
            <Badge size="sm" variant="success">
              Up to date
            </Badge>
          ) : null}
          <Button
            disabled={system.isPending}
            loading={check.isPending}
            onClick={() => check.mutate()}
            size="sm"
            variant="outline"
          >
            <RefreshCwIcon />
            Check updates
          </Button>
        </div>
      }
      description="Compare this instance with GitHub main and copy the host update command."
      icon={<RefreshCwIcon className="size-4" />}
      id="updates"
      title="Updates"
    >
      {system.isError ? <ErrorText className="mt-4">{system.error.message}</ErrorText> : null}

      <dl className="mt-4 divide-y text-sm">
        <FactRow label="Mode">{selfHosted ? "Self-hosted" : "Cloud"}</FactRow>
        <FactRow label="Current" mono>
          {shortSha(latest?.currentCommitSha ?? system.data?.currentCommitSha)}
        </FactRow>
        <FactRow label="Latest" mono>
          {shortSha(latest?.latestCommitSha)}
        </FactRow>
        <FactRow label="Status">
          <span className="whitespace-normal font-normal text-muted-foreground text-sm">
            {latest
              ? latest.message
              : selfHosted
                ? "Run a check to compare this instance with GitHub main."
                : "Cloud instances are updated by the deploy pipeline."}
          </span>
        </FactRow>
      </dl>

      {selfHosted ? (
        <div className="mt-4 flex items-center gap-2 rounded-md border bg-muted/20 py-1.5 pr-1.5 pl-3">
          <code className="min-w-0 flex-1 truncate font-mono text-xs">
            <span className="select-none text-muted-foreground">$ </span>
            {command}
          </code>
          <Button
            aria-label="Copy update command"
            onClick={copyCommand}
            size="icon-sm"
            variant="ghost"
          >
            {copied ? <CheckIcon /> : <CopyIcon />}
          </Button>
        </div>
      ) : null}
    </SettingsCard>
  );
}

const ROLE_BADGE_VARIANT: Record<WorkspaceRole, "info" | "secondary" | "outline"> = {
  owner: "info",
  admin: "secondary",
  member: "outline",
};

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

  const memberCount = team.data?.members.length ?? 0;
  const pendingCount = team.data?.invitations.length ?? 0;

  return (
    <SettingsCard
      action={
        team.isSuccess ? (
          <Badge size="sm" variant="outline">
            {memberCount} {memberCount === 1 ? "member" : "members"}
            {pendingCount > 0 ? ` · ${pendingCount} pending` : ""}
          </Badge>
        ) : undefined
      }
      description="Invite teammates and control workspace access."
      icon={<UsersIcon className="size-4" />}
      id="team"
      title="Team"
    >
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

      <div className="mt-5">
        {team.isPending ? (
          <div className="h-16 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
        ) : team.isError ? (
          <ErrorText>{team.error.message}</ErrorText>
        ) : memberCount === 0 && pendingCount === 0 ? (
          <EmptyNote>No team members yet.</EmptyNote>
        ) : (
          <ul className="divide-y overflow-hidden rounded-lg border bg-background/40">
            {team.data.members.map((member) => (
              <li
                className="group flex items-center justify-between gap-3 px-3 py-2.5 transition-colors hover:bg-muted/40"
                key={member.id}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <Avatar name={member.name || member.email} />
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <p className="truncate font-medium text-sm">{member.name || member.email}</p>
                      <Badge size="sm" variant={ROLE_BADGE_VARIANT[member.role]}>
                        {member.role}
                      </Badge>
                    </div>
                    <p className="truncate text-muted-foreground text-xs">{member.email}</p>
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <RoleSelect
                    onChange={(nextRole) => updateRole.mutate({ id: member.id, role: nextRole })}
                    value={member.role}
                  />
                  <Button
                    aria-label={`Remove ${member.email}`}
                    loading={removeMember.isPending && removeMember.variables === member.id}
                    onClick={() => {
                      if (window.confirm(`Remove ${member.email} from this workspace?`)) {
                        removeMember.mutate(member.id);
                      }
                    }}
                    size="icon-sm"
                    variant="ghost"
                  >
                    <TrashIcon />
                  </Button>
                </div>
              </li>
            ))}
            {team.data.invitations.map((invitation) => (
              <li
                className="flex items-center justify-between gap-3 bg-muted/20 px-3 py-2.5"
                key={invitation.id}
              >
                <div className="flex min-w-0 items-center gap-3">
                  <span className="flex size-8 shrink-0 items-center justify-center rounded-full border border-dashed text-muted-foreground">
                    <UsersIcon className="size-3.5" />
                  </span>
                  <div className="min-w-0">
                    <div className="flex items-center gap-2">
                      <p className="truncate font-medium text-sm">{invitation.email}</p>
                      <Badge size="sm" variant="warning">
                        pending
                      </Badge>
                    </div>
                    <p className="truncate text-muted-foreground text-xs">
                      invited as {invitation.role}
                    </p>
                  </div>
                </div>
                <Button
                  loading={cancelInvite.isPending && cancelInvite.variables === invitation.id}
                  onClick={() => cancelInvite.mutate(invitation.id)}
                  size="sm"
                  variant="outline"
                >
                  Cancel
                </Button>
              </li>
            ))}
          </ul>
        )}
      </div>
    </SettingsCard>
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
    <SettingsCard
      action={
        list.length > 0 ? (
          <Badge size="sm" variant="success">
            {list.length} connected
          </Badge>
        ) : undefined
      }
      description="Connect provider APIs so Basse can create and sync managed load balancers for multi-server apps."
      icon={<NetworkIcon className="size-4" />}
      id="traffic"
      title="Traffic providers"
    >
      <div className="mt-4">
        {integrations.isPending ? (
          <div className="h-16 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
        ) : list.length === 0 ? (
          <EmptyNote>No traffic providers connected.</EmptyNote>
        ) : (
          <ul className="divide-y overflow-hidden rounded-lg border bg-background/40">
            {list.map((integration) => (
              <li
                key={integration.id}
                className="group flex items-center justify-between gap-3 px-3 py-2.5 transition-colors hover:bg-muted/40"
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
                  className="opacity-0 transition-opacity focus-visible:opacity-100 group-hover:opacity-100"
                  loading={remove.isPending && remove.variables === integration.id}
                  onClick={() => {
                    if (window.confirm(`Remove ${integration.name}?`)) {
                      remove.mutate(integration.id);
                    }
                  }}
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

      <form className="mt-5 space-y-4 border-t pt-4" onSubmit={handleSubmit}>
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
              autoComplete="off"
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
        {error ? <ErrorText>{error}</ErrorText> : null}
        <Button disabled={!token.trim()} loading={save.isPending} size="sm" type="submit">
          Connect {providerLabel}
        </Button>
      </form>
    </SettingsCard>
  );
}

function trafficProviderLabel(provider: LoadBalancerProvider): string {
  return provider === "hetzner" ? "Hetzner" : "Cloudflare";
}
