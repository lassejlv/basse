import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  CheckIcon,
  ContainerIcon,
  CopyIcon,
  ExternalLinkIcon,
  FingerprintIcon,
  GitBranchIcon,
  KeyRoundIcon,
  LockIcon,
  PlusIcon,
  ShieldCheckIcon,
  TriangleAlertIcon,
} from "lucide-react";
import { FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import type { ApiTokenScope } from "@basse/shared";
import { useClipboard } from "@/components/app/shared";
import { EmptyNote, ErrorText, RowDeleteButton, SectionLabel } from "@/components/dashboard";
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
  DialogTrigger,
} from "@/components/ui/dialog";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
import { createApiToken, deleteApiToken, listApiTokens } from "@/lib/api-tokens";
import { authClient } from "@/lib/auth-client";
import { disconnectDepot, getDepotConnection, saveDepotConnection } from "@/lib/depot";
import { disconnectNeon, getNeonConnection, saveNeonConnection } from "@/lib/neon";
import { NeonIcon } from "@/components/database-icon";
import {
  completeGitHubAppManifest,
  deleteGitHubAppInstallation,
  disconnectGitHubApp,
  getGitHubAppIntegration,
  getGitHubAppManifest,
  listGitHubAppInstallations,
  saveGitHubAppInstallation,
  syncGitHubAppInstallations,
} from "@/lib/github";
import { toast, toMessage } from "@/lib/toast";
import { createSshKey, deleteSshKey, listSshKeys } from "@/lib/ssh-keys";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_authed/secrets")({
  validateSearch: (search: Record<string, unknown>) => ({
    code: typeof search.code === "string" ? search.code : undefined,
    state: typeof search.state === "string" ? search.state : undefined,
    installation_id:
      typeof search.installation_id === "string" ? search.installation_id : undefined,
    setup_action: typeof search.setup_action === "string" ? search.setup_action : undefined,
  }),
  component: SecretsRoute,
});

/* Presentation helpers that give credentials a vault-entry feel:
   masked mono values, dot statuses, and relative "last used" stamps. */

function relativeTime(iso: string): string {
  const diff = Date.now() - new Date(iso).getTime();
  if (diff < 60_000) return "just now";
  const minutes = Math.floor(diff / 60_000);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.floor(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  const days = Math.floor(hours / 24);
  if (days < 30) return `${days}d ago`;
  return new Date(iso).toLocaleDateString();
}

function StatusDot({
  tone,
  pulse,
}: {
  tone: "success" | "muted" | "warning" | "destructive";
  pulse?: boolean;
}) {
  return (
    <span className="relative inline-flex size-1.5 shrink-0">
      {pulse ? (
        <span
          className={cn(
            "absolute inline-flex size-full animate-ping rounded-full opacity-40",
            tone === "success" && "bg-success",
          )}
          aria-hidden
        />
      ) : null}
      <span
        className={cn(
          "relative inline-flex size-1.5 rounded-full",
          tone === "success" && "bg-success",
          tone === "muted" && "bg-muted-foreground/40",
          tone === "warning" && "bg-warning",
          tone === "destructive" && "bg-destructive",
        )}
      />
    </span>
  );
}

function MaskedValue({ prefix, hint }: { prefix?: string; hint?: string }) {
  return (
    <span className="inline-flex items-baseline gap-0.5 font-mono text-muted-foreground text-xs tabular-nums">
      {prefix ? <span className="text-foreground/70">{prefix}</span> : null}
      <span className="tracking-[0.2em]" aria-hidden>
        ••••••••
      </span>
      {hint ? <span className="text-foreground/70">{hint}</span> : null}
    </span>
  );
}

function VaultRow({
  status,
  action,
  children,
}: {
  status?: ReactNode;
  action?: ReactNode;
  children: ReactNode;
}) {
  return (
    <li className="group flex items-center gap-3 px-3 py-2.5 transition-colors hover:bg-muted/40">
      {status}
      <div className="min-w-0 flex-1">{children}</div>
      {action ? (
        <div className="opacity-0 transition-opacity focus-within:opacity-100 group-hover:opacity-100">
          {action}
        </div>
      ) : null}
    </li>
  );
}

function VaultList({ children }: { children: ReactNode }) {
  return (
    <ul className="divide-y overflow-hidden rounded-lg border bg-background/40">{children}</ul>
  );
}

function SecretsRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <header>
        <div className="flex items-center gap-2">
          <SectionLabel>Workspace / Vault</SectionLabel>
        </div>
        <div className="mt-1 flex items-center gap-2.5">
          <h1 className="font-semibold text-2xl tracking-tight md:text-3xl">Secrets</h1>
          <LockIcon aria-hidden className="size-4 text-muted-foreground" />
        </div>
        <p className="mt-1 text-muted-foreground text-sm">
          Credentials for {activeOrganization?.name ?? "this workspace"} — stored encrypted, values
          shown once, fingerprints only after that.
        </p>
        <VaultSummary organizationId={organizationId} />
      </header>

      <div className="max-w-5xl">
        <SectionLabel as="h2">Credentials</SectionLabel>
        <div className="mt-3 grid items-start gap-4 xl:grid-cols-2">
          <ApiTokensSection organizationId={organizationId} />
          <SshKeysSection organizationId={organizationId} />
        </div>
      </div>

      <div className="max-w-5xl">
        <SectionLabel as="h2">Integrations</SectionLabel>
        <div className="mt-3 grid items-start gap-4 xl:grid-cols-2">
          <GitHubSection organizationId={organizationId} />
          <div className="flex flex-col gap-4">
            <DepotSection organizationId={organizationId} />
            <NeonSection organizationId={organizationId} />
          </div>
        </div>
      </div>
    </section>
  );
}

function VaultSummary({ organizationId }: { organizationId?: string }) {
  const enabled = Boolean(organizationId);
  const tokens = useQuery({
    queryKey: ["api-tokens", organizationId],
    queryFn: listApiTokens,
    enabled,
  });
  const keys = useQuery({
    queryKey: ["ssh-keys", organizationId],
    queryFn: listSshKeys,
    enabled,
  });
  const github = useQuery({
    queryKey: ["github-app-integration", organizationId],
    queryFn: getGitHubAppIntegration,
    enabled,
  });
  const depot = useQuery({
    queryKey: ["depot-connection", organizationId],
    queryFn: getDepotConnection,
    enabled,
  });
  const neon = useQuery({
    queryKey: ["neon-connection", organizationId],
    queryFn: getNeonConnection,
    enabled,
  });

  const connectedCount = [
    github.data?.connected,
    depot.data?.connected,
    neon.data?.connected,
  ].filter(Boolean).length;

  const stats: { label: string; value: string; tone: "success" | "muted" }[] = [
    {
      label: "API tokens",
      value: String(tokens.data?.length ?? "—"),
      tone: (tokens.data?.length ?? 0) > 0 ? "success" : "muted",
    },
    {
      label: "SSH keys",
      value: String(keys.data?.length ?? "—"),
      tone: (keys.data?.length ?? 0) > 0 ? "success" : "muted",
    },
    {
      label: "integrations",
      value: `${connectedCount}/3`,
      tone: connectedCount > 0 ? "success" : "muted",
    },
  ];

  return (
    <dl className="mt-4 flex flex-wrap items-center gap-x-5 gap-y-2 font-mono text-xs">
      {stats.map((stat) => (
        <div className="flex items-center gap-2" key={stat.label}>
          <StatusDot tone={stat.tone} />
          <dd className="text-foreground tabular-nums">{stat.value}</dd>
          <dt className="text-muted-foreground">{stat.label}</dt>
        </div>
      ))}
      <div className="flex items-center gap-1.5 text-muted-foreground">
        <ShieldCheckIcon aria-hidden className="size-3.5" />
        encrypted at rest
      </div>
    </dl>
  );
}

const API_TOKEN_SCOPES: { value: ApiTokenScope; label: string; description: string }[] = [
  {
    value: "read",
    label: "Read",
    description: "List workspace resources and deployment state.",
  },
  {
    value: "deployments:write",
    label: "Deploy",
    description: "Trigger and roll back deployments.",
  },
  {
    value: "write",
    label: "Write",
    description: "Mutate workspace resources.",
  },
];

const SCOPE_LABELS: Record<ApiTokenScope, string> = {
  read: "read",
  "deployments:write": "deploy",
  write: "write",
};

function tokenExpiry(expiresAt: string | null): {
  tone: "success" | "warning" | "destructive";
  note?: string;
} {
  if (!expiresAt) return { tone: "success" };
  const remaining = new Date(expiresAt).getTime() - Date.now();
  if (remaining <= 0) return { tone: "destructive", note: "expired" };
  if (remaining < 7 * 24 * 60 * 60 * 1000) {
    return {
      tone: "warning",
      note: `expires ${new Date(expiresAt).toLocaleDateString()}`,
    };
  }
  return { tone: "success" };
}

function ApiTokensSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [expiresAt, setExpiresAt] = useState("");
  const [scopes, setScopes] = useState<ApiTokenScope[]>(["read", "deployments:write"]);
  const [createdToken, setCreatedToken] = useState<string | null>(null);
  const { copiedId, copy } = useClipboard();
  const tokensKey = ["api-tokens", organizationId];

  const tokens = useQuery({
    queryKey: tokensKey,
    queryFn: listApiTokens,
    enabled: Boolean(organizationId),
  });

  const create = useMutation({
    mutationFn: () =>
      createApiToken({
        name,
        scopes,
        expiresAt: expiresAt ? new Date(expiresAt).toISOString() : null,
      }),
    onSuccess: async (result) => {
      setCreatedToken(result.token);
      setName("");
      setExpiresAt("");
      toast.success("API token created");
      await queryClient.invalidateQueries({ queryKey: tokensKey });
    },
    onError: (error) => toast.error("Couldn't create API token", { description: toMessage(error) }),
  });

  const remove = useMutation({
    mutationFn: deleteApiToken,
    onSuccess: async () => {
      toast.success("API token deleted");
      await queryClient.invalidateQueries({ queryKey: tokensKey });
    },
    onError: (error) => toast.error("Couldn't delete API token", { description: toMessage(error) }),
  });

  function toggleScope(scope: ApiTokenScope, checked: boolean) {
    setScopes((current) => {
      const next = checked ? [...new Set([...current, scope])] : current.filter((s) => s !== scope);
      return next.length > 0 ? next : current;
    });
  }

  const tokenList = tokens.data ?? [];

  return (
    <SectionCard
      badge={<Badge size="sm">Automation</Badge>}
      description="Scoped bearer tokens for scripts, CI, and deploy automation."
      icon={<KeyRoundIcon className="size-4" />}
      title="API tokens"
    >
      <div className="mt-4">
        {tokenList.length > 0 ? (
          <VaultList>
            {tokenList.map((token) => {
              const expiry = tokenExpiry(token.expiresAt);
              return (
                <VaultRow
                  action={
                    <RowDeleteButton
                      confirmMessage={`Revoke ${token.name}? Anything using it loses access immediately.`}
                      label={`Revoke ${token.name}`}
                      loading={remove.isPending && remove.variables === token.id}
                      onClick={() => remove.mutate(token.id)}
                    />
                  }
                  key={token.id}
                  status={<StatusDot tone={expiry.tone} />}
                >
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <p className="truncate font-medium text-sm">{token.name}</p>
                    {token.scopes.map((scope) => (
                      <Badge key={scope} size="sm" variant="outline">
                        {SCOPE_LABELS[scope]}
                      </Badge>
                    ))}
                    {expiry.note ? (
                      <Badge
                        size="sm"
                        variant={expiry.tone === "destructive" ? "error" : "warning"}
                      >
                        {expiry.note}
                      </Badge>
                    ) : null}
                  </div>
                  <p className="mt-0.5 flex flex-wrap items-center gap-x-2 text-muted-foreground text-xs">
                    <MaskedValue prefix={token.tokenPrefix} />
                    <span>
                      {token.lastUsedAt
                        ? `last used ${relativeTime(token.lastUsedAt)}`
                        : "never used"}
                    </span>
                  </p>
                </VaultRow>
              );
            })}
          </VaultList>
        ) : null}
        {tokens.isSuccess && tokenList.length === 0 ? (
          <EmptyNote>No API tokens yet.</EmptyNote>
        ) : null}
        {tokens.isError ? (
          <ErrorText>Couldn't load API tokens: {toMessage(tokens.error)}</ErrorText>
        ) : null}
      </div>

      <Dialog
        open={open}
        onOpenChange={(next) => {
          setOpen(next);
          if (!next) setCreatedToken(null);
        }}
      >
        <DialogTrigger
          render={
            <Button className="mt-4" size="sm">
              <PlusIcon />
              New token
            </Button>
          }
        />
        <DialogPopup>
          <DialogPanel>
            <DialogHeader>
              <DialogTitle>Create API token</DialogTitle>
              <DialogDescription>
                The token is only shown once. Store it somewhere safe before closing this dialog.
              </DialogDescription>
            </DialogHeader>
            {createdToken ? (
              <div className="space-y-3">
                <div className="rounded-lg border border-warning/40 border-dashed bg-warning/8 p-3">
                  <p className="flex items-center gap-1.5 font-medium text-warning-foreground text-xs">
                    <TriangleAlertIcon aria-hidden className="size-3.5" />
                    One-time reveal — this value is never shown again.
                  </p>
                  <Textarea
                    className="mt-2 font-mono text-xs"
                    id="created-api-token"
                    readOnly
                    value={createdToken}
                  />
                </div>
                <Button onClick={() => copy("created-token", createdToken)} type="button">
                  {copiedId === "created-token" ? <CheckIcon /> : <CopyIcon />}
                  {copiedId === "created-token" ? "Copied" : "Copy token"}
                </Button>
              </div>
            ) : (
              <form
                className="space-y-4"
                onSubmit={(event) => {
                  event.preventDefault();
                  create.mutate();
                }}
              >
                <div className="space-y-2">
                  <Label htmlFor="api-token-name">Name</Label>
                  <Input
                    id="api-token-name"
                    onChange={(event) => setName(event.currentTarget.value)}
                    placeholder="GitHub Actions deploy"
                    value={name}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="api-token-expiry">Expires at</Label>
                  <Input
                    id="api-token-expiry"
                    onChange={(event) => setExpiresAt(event.currentTarget.value)}
                    type="datetime-local"
                    value={expiresAt}
                  />
                </div>
                <div className="space-y-2">
                  <Label>Scopes</Label>
                  <div className="space-y-2">
                    {API_TOKEN_SCOPES.map((scope) => (
                      <label
                        className="flex items-start gap-2 rounded-md border px-3 py-2 text-sm has-checked:border-primary/40 has-checked:bg-muted/40"
                        key={scope.value}
                      >
                        <input
                          checked={scopes.includes(scope.value)}
                          className="mt-1"
                          onChange={(event) =>
                            toggleScope(scope.value, event.currentTarget.checked)
                          }
                          type="checkbox"
                        />
                        <span>
                          <span className="block font-medium">{scope.label}</span>
                          <span className="block text-muted-foreground text-xs">
                            {scope.description}
                          </span>
                        </span>
                      </label>
                    ))}
                  </div>
                </div>
                <DialogFooter>
                  <DialogClose render={<Button variant="outline">Cancel</Button>} />
                  <Button loading={create.isPending} type="submit">
                    Create token
                  </Button>
                </DialogFooter>
              </form>
            )}
          </DialogPanel>
        </DialogPopup>
      </Dialog>
    </SectionCard>
  );
}

function SectionCard({
  icon,
  title,
  description,
  badge,
  children,
}: {
  icon: ReactNode;
  title: string;
  description: string;
  badge?: ReactNode;
  children: ReactNode;
}) {
  return (
    <Card className="p-5">
      <div className="flex items-start gap-3">
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-muted/30 text-foreground/80 shadow-[inset_0_1px_0_--theme(--color-white/6%)]">
          {icon}
        </span>
        <div className="min-w-0 flex-1">
          <div className="flex flex-wrap items-center gap-2">
            <h2 className="font-semibold text-sm">{title}</h2>
            {badge}
          </div>
          <p className="mt-0.5 text-muted-foreground text-xs leading-relaxed">{description}</p>
        </div>
      </div>
      {children}
    </Card>
  );
}

function ConnectionBadge({ connected }: { connected: boolean | undefined }) {
  return connected ? (
    <Badge size="sm" variant="success">
      <StatusDot pulse tone="success" />
      Connected
    </Badge>
  ) : (
    <Badge size="sm" variant="secondary">
      Not connected
    </Badge>
  );
}

function GitHubSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const search = Route.useSearch();
  const processedCode = useRef<string | null>(null);
  const processedInstallation = useRef<string | null>(null);
  const processedSetupCallback = useRef<string | null>(null);
  const { copiedId, copy } = useClipboard();
  const [startingGitHubSetup, setStartingGitHubSetup] = useState(false);
  const integrationKey = ["github-app-integration", organizationId];
  const installationsKey = ["github-app-installations", organizationId];

  function invalidate(...keys: unknown[][]) {
    return Promise.all(keys.map((queryKey) => queryClient.invalidateQueries({ queryKey })));
  }

  function refreshInstallations() {
    return invalidate(installationsKey, ["github-repositories"]);
  }

  const integration = useQuery({
    queryKey: integrationKey,
    queryFn: getGitHubAppIntegration,
    enabled: Boolean(organizationId),
  });

  const manifest = useQuery({
    queryKey: ["github-app-manifest", organizationId],
    queryFn: getGitHubAppManifest,
    enabled: Boolean(organizationId),
  });

  const installations = useQuery({
    queryKey: installationsKey,
    queryFn: listGitHubAppInstallations,
    enabled: Boolean(organizationId),
  });

  const completeManifest = useMutation({
    mutationFn: (input: { code: string; state: string }) => completeGitHubAppManifest(input),
    onSuccess: async () => {
      toast.success("GitHub App connected");
      await invalidate(integrationKey, installationsKey, ["github-repositories"]);
      await clearGitHubCallbackSearch(navigate);
    },
    onError: (error) =>
      toast.error("Couldn't connect GitHub App", { description: toMessage(error) }),
  });

  const saveInstallation = useMutation({
    mutationFn: (input: { installationId: string; setupAction?: string }) =>
      saveGitHubAppInstallation({ installationId: input.installationId }),
    onSuccess: async (_installation, input) => {
      toast.success(
        input.setupAction === "update"
          ? "GitHub repository access updated"
          : "GitHub installation saved",
      );
      await refreshInstallations();
      await clearGitHubCallbackSearch(navigate);
    },
    onError: (error) =>
      toast.error("Couldn't save GitHub installation", { description: toMessage(error) }),
  });

  const syncInstallations = useMutation({
    mutationFn: syncGitHubAppInstallations,
    onSuccess: async (synced) => {
      if (synced.length > 0) {
        toast.success("GitHub installation saved");
        await refreshInstallations();
        await clearGitHubCallbackSearch(navigate);
        return;
      }

      toast.warning("GitHub installation is not active yet", {
        description:
          "If this was an organization install request, an owner may still need to approve it.",
      });
    },
    onError: (error) =>
      toast.error("Couldn't sync GitHub installations", { description: toMessage(error) }),
  });

  const disconnect = useMutation({
    mutationFn: disconnectGitHubApp,
    onSuccess: async () => {
      toast.success("GitHub disconnected");
      await invalidate(integrationKey, installationsKey, ["github-repositories"]);
    },
    onError: (error) =>
      toast.error("Couldn't disconnect GitHub", { description: toMessage(error) }),
  });

  const removeInstallation = useMutation({
    mutationFn: deleteGitHubAppInstallation,
    onSuccess: async () => {
      toast.success("GitHub installation removed");
      await refreshInstallations();
    },
    onError: (error) =>
      toast.error("Couldn't remove GitHub installation", { description: toMessage(error) }),
  });

  useEffect(() => {
    if (!organizationId || !search.code || processedCode.current === search.code) return;
    processedCode.current = search.code;
    completeManifest.mutate({ code: search.code, state: search.state ?? "" });
  }, [completeManifest, organizationId, search.code, search.state]);

  useEffect(() => {
    if (
      !organizationId ||
      !search.installation_id ||
      processedInstallation.current === search.installation_id
    ) {
      return;
    }
    if (!/^\d+$/.test(search.installation_id)) return;
    processedInstallation.current = search.installation_id;
    saveInstallation.mutate({
      installationId: search.installation_id,
      setupAction: search.setup_action,
    });
  }, [organizationId, saveInstallation, search.installation_id, search.setup_action]);

  useEffect(() => {
    if (!organizationId || !search.setup_action || search.code || search.installation_id) return;
    const key = search.setup_action;
    if (processedSetupCallback.current === key) return;
    processedSetupCallback.current = key;
    syncInstallations.mutate();
  }, [organizationId, search.code, search.installation_id, search.setup_action, syncInstallations]);

  const connected = integration.data?.connected;
  const installUrl = integration.data?.installUrl;
  const webhookUrl = integration.data?.webhookUrl ?? manifest.data?.webhookUrl;
  const savedInstallations = installations.data ?? [];
  const installCallbackMissingId =
    Boolean(search.setup_action) && !search.installation_id && !search.code;

  async function submitGitHubManifest(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (
      connected &&
      !window.confirm(
        "Replace the connected GitHub App? This clears saved installations for this workspace.",
      )
    ) {
      return;
    }

    setStartingGitHubSetup(true);
    try {
      const result = await manifest.refetch();
      const data = result.data;
      if (!data?.actionUrl || !data.manifest) {
        throw result.error ?? new Error("GitHub App manifest is unavailable");
      }

      const form = document.createElement("form");
      form.action = data.actionUrl;
      form.method = "post";
      const input = document.createElement("input");
      input.name = "manifest";
      input.type = "hidden";
      input.value = data.manifest;
      form.append(input);
      document.body.append(form);
      form.submit();
    } catch (error) {
      setStartingGitHubSetup(false);
      toast.error("Couldn't prepare GitHub App setup", { description: toMessage(error) });
    }
  }

  return (
    <SectionCard
      badge={<ConnectionBadge connected={connected} />}
      description="Create a workspace GitHub App, install it on private repositories, and deploy over short-lived installation tokens."
      icon={<GitBranchIcon className="size-4.5" />}
      title="GitHub"
    >
      {integration.isError ? (
        <ErrorText className="mt-4">
          Couldn't load GitHub integration: {toMessage(integration.error)}
        </ErrorText>
      ) : null}
      {manifest.isError ? (
        <ErrorText className="mt-4">
          Couldn't prepare GitHub App setup: {toMessage(manifest.error)}
        </ErrorText>
      ) : null}
      {installations.isError ? (
        <ErrorText className="mt-4">
          Couldn't load GitHub installations: {toMessage(installations.error)}
        </ErrorText>
      ) : null}
      {installCallbackMissingId ? (
        <p className="mt-4 text-muted-foreground text-sm">
          GitHub returned from setup without an installation id. Choose an account and repository
          access on GitHub, then complete the installation.
        </p>
      ) : null}

      {connected ? (
        <div className="mt-5 flex items-center gap-3 rounded-lg border bg-background/40 px-3 py-2.5 text-sm">
          <StatusDot pulse tone="success" />
          <div className="min-w-0">
            <p className="truncate font-medium">{integration.data?.appName}</p>
            <p className="truncate font-mono text-muted-foreground text-xs">
              github.com/apps/{integration.data?.appSlug}
            </p>
          </div>
        </div>
      ) : null}

      {connected && webhookUrl ? (
        <div className="mt-4 space-y-2">
          <Label htmlFor="github-webhook-url">Webhook URL</Label>
          <div className="flex gap-2">
            <Input
              className="font-mono text-xs"
              id="github-webhook-url"
              readOnly
              value={webhookUrl}
            />
            <Button
              aria-label="Copy GitHub webhook URL"
              onClick={() => copy("webhook", webhookUrl)}
              size="icon"
              type="button"
              variant="outline"
            >
              {copiedId === "webhook" ? <CheckIcon /> : <CopyIcon />}
            </Button>
          </div>
        </div>
      ) : null}

      {savedInstallations.length > 0 ? (
        <div className="mt-4">
          <SectionLabel as="h3" className="mb-2">
            Installations
          </SectionLabel>
          <VaultList>
            {savedInstallations.map((installation) => (
              <VaultRow
                action={
                  <RowDeleteButton
                    confirmMessage={`Remove ${installation.accountLogin} from this workspace's GitHub installations?`}
                    label={`Remove ${installation.accountLogin} installation`}
                    loading={removeInstallation.isPending}
                    onClick={() => removeInstallation.mutate(installation.id)}
                  />
                }
                key={installation.id}
                status={<StatusDot tone="success" />}
              >
                <p className="truncate font-medium text-sm">{installation.accountLogin}</p>
                <p className="truncate text-muted-foreground text-xs">
                  {installation.accountType ?? "account"} ·{" "}
                  {installation.repositorySelection ?? "repositories"}
                </p>
              </VaultRow>
            ))}
          </VaultList>
        </div>
      ) : connected ? (
        <EmptyNote className="mt-4">No GitHub installations saved yet.</EmptyNote>
      ) : null}

      <div className="mt-5 flex flex-wrap gap-2 border-t pt-4">
        <form action={manifest.data?.actionUrl} method="post" onSubmit={submitGitHubManifest}>
          <input name="manifest" type="hidden" value={manifest.data?.manifest ?? ""} />
          <Button
            disabled={!organizationId || manifest.isPending || startingGitHubSetup}
            loading={startingGitHubSetup}
            size="sm"
            type="submit"
            variant={connected ? "outline" : "default"}
          >
            {connected ? "Replace GitHub App" : "Create GitHub App"}
          </Button>
        </form>

        {installUrl ? (
          <>
            <Button render={<a href={installUrl} />} size="sm" variant="outline">
              {savedInstallations.length > 0 ? "Install or update access" : "Install app"}
              <ExternalLinkIcon />
            </Button>
            <Button
              loading={disconnect.isPending}
              onClick={() => {
                if (window.confirm("Disconnect GitHub from this workspace?")) {
                  disconnect.mutate();
                }
              }}
              size="sm"
              variant="outline"
            >
              Disconnect
            </Button>
          </>
        ) : null}
      </div>
    </SectionCard>
  );
}

async function clearGitHubCallbackSearch(navigate: ReturnType<typeof useNavigate>) {
  await navigate({
    replace: true,
    search: {
      code: undefined,
      installation_id: undefined,
      setup_action: undefined,
      state: undefined,
    },
    to: "/secrets",
  });
}

function splitPublicKey(publicKey: string): { algo: string; tail: string } {
  const [algo, body] = publicKey.trim().split(/\s+/);
  return {
    algo: algo && algo.startsWith("ssh-") ? algo.slice(4) : (algo ?? "key"),
    tail: body ? body.slice(-16) : "",
  };
}

function SshKeysSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["ssh-keys", organizationId];
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [privateKey, setPrivateKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const keys = useQuery({
    queryKey,
    queryFn: listSshKeys,
    enabled: Boolean(organizationId),
  });

  const addKey = useMutation({
    mutationFn: () => createSshKey({ name, privateKey }),
    onSuccess: async () => {
      setName("");
      setPrivateKey("");
      setError(null);
      setOpen(false);
      toast.success("SSH key added");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const removeKey = useMutation({
    mutationFn: (id: string) => deleteSshKey(id),
    onSuccess: async () => {
      toast.success("Key removed");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (error) => toast.error("Couldn't remove key", { description: toMessage(error) }),
  });

  const keyList = keys.data ?? [];

  return (
    <SectionCard
      badge={
        keyList.length > 0 ? (
          <Badge size="sm" variant="outline">
            {keyList.length}
          </Badge>
        ) : undefined
      }
      description="Private keys Basse can use when connecting to servers in this workspace."
      icon={<FingerprintIcon className="size-4.5" />}
      title="SSH keys"
    >
      <div className="mt-5">
        {keys.isPending ? (
          <div className="h-16 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
        ) : keyList.length === 0 ? (
          <EmptyNote>No SSH keys yet. Add one to connect servers with an existing key.</EmptyNote>
        ) : (
          <VaultList>
            {keyList.map((key) => {
              const fingerprint = splitPublicKey(key.publicKey);
              return (
                <VaultRow
                  action={
                    <RowDeleteButton
                      confirmMessage={`Delete ${key.name}? Servers using it can no longer be reached with this key.`}
                      label={`Delete ${key.name}`}
                      loading={removeKey.isPending && removeKey.variables === key.id}
                      onClick={() => removeKey.mutate(key.id)}
                    />
                  }
                  key={key.id}
                  status={<StatusDot tone={key.hasPrivateKey ? "success" : "muted"} />}
                >
                  <div className="flex flex-wrap items-center gap-x-2 gap-y-1">
                    <p className="truncate font-medium text-sm">{key.name}</p>
                    <Badge size="sm" variant="outline">
                      {fingerprint.algo}
                    </Badge>
                    {key.hasPrivateKey ? null : (
                      <Badge size="sm" variant="secondary">
                        public only
                      </Badge>
                    )}
                  </div>
                  <p className="mt-0.5 truncate font-mono text-muted-foreground text-xs">
                    …{fingerprint.tail}
                  </p>
                </VaultRow>
              );
            })}
          </VaultList>
        )}
      </div>

      <div className="mt-4">
        <Dialog
          open={open}
          onOpenChange={(next) => {
            setOpen(next);
            if (!next) setError(null);
          }}
        >
          <DialogTrigger
            render={
              <Button size="sm" variant="outline">
                <PlusIcon />
                Add SSH key
              </Button>
            }
          />
          <DialogPopup className="h-fit max-w-md">
            <form
              onSubmit={(event) => {
                event.preventDefault();
                addKey.mutate();
              }}
            >
              <DialogHeader>
                <DialogTitle>Add SSH key</DialogTitle>
                <DialogDescription>
                  Basse stores it encrypted and derives the public key automatically.
                </DialogDescription>
              </DialogHeader>
              <DialogPanel className="space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="ssh-key-name">Name</Label>
                  <Input
                    autoFocus
                    id="ssh-key-name"
                    onChange={(event) => setName(event.currentTarget.value)}
                    placeholder="laptop"
                    required
                    value={name}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="ssh-key-private">Private key</Label>
                  <Textarea
                    className="font-mono text-xs"
                    id="ssh-key-private"
                    onChange={(event) => setPrivateKey(event.currentTarget.value)}
                    placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
                    required
                    rows={6}
                    value={privateKey}
                  />
                </div>
                {error ? <ErrorText>{error}</ErrorText> : null}
              </DialogPanel>
              <DialogFooter>
                <DialogClose render={<Button variant="outline">Cancel</Button>} />
                <Button disabled={!organizationId} loading={addKey.isPending} type="submit">
                  Add SSH key
                </Button>
              </DialogFooter>
            </form>
          </DialogPopup>
        </Dialog>
      </div>
    </SectionCard>
  );
}

function NeonSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["neon-connection", organizationId];
  const [open, setOpen] = useState(false);
  const [apiKey, setApiKey] = useState("");
  const [error, setError] = useState<string | null>(null);

  const connection = useQuery({
    queryKey,
    queryFn: getNeonConnection,
    enabled: Boolean(organizationId),
  });

  const save = useMutation({
    mutationFn: () => saveNeonConnection({ apiKey }),
    onSuccess: async () => {
      setApiKey("");
      setError(null);
      setOpen(false);
      toast.success("Neon API key saved");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const disconnect = useMutation({
    mutationFn: disconnectNeon,
    onSuccess: async () => {
      toast.success("Neon disconnected");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (error) => toast.error("Couldn't disconnect Neon", { description: toMessage(error) }),
  });

  const connected = connection.data?.connected;

  return (
    <SectionCard
      badge={<ConnectionBadge connected={connected} />}
      description="Provision serverless Postgres databases on Neon from the app palette."
      icon={<NeonIcon className="size-4.5" />}
      title="Neon"
    >
      {connected ? (
        <div className="mt-5 flex items-center gap-3 rounded-lg border bg-background/40 px-3 py-2.5">
          <StatusDot pulse tone="success" />
          <div className="min-w-0">
            <p className="font-medium text-sm">API key</p>
            <MaskedValue hint={connection.data?.keyHint} />
          </div>
        </div>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <Dialog
          open={open}
          onOpenChange={(next) => {
            setOpen(next);
            if (!next) setError(null);
          }}
        >
          <DialogTrigger
            render={
              <Button size="sm" variant={connected ? "outline" : "default"}>
                {connected ? "Update API key" : "Connect Neon"}
              </Button>
            }
          />
          <DialogPopup className="h-fit max-w-md">
            <form
              onSubmit={(event) => {
                event.preventDefault();
                save.mutate();
              }}
            >
              <DialogHeader>
                <DialogTitle>{connected ? "Update Neon API key" : "Connect Neon"}</DialogTitle>
                <DialogDescription>
                  Create an API key in the Neon console under Account settings → API keys. New
                  databases are provisioned as Neon projects on your account.
                </DialogDescription>
              </DialogHeader>
              <DialogPanel className="space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="neon-api-key">API key</Label>
                  <Input
                    autoComplete="off"
                    autoFocus
                    id="neon-api-key"
                    onChange={(event) => setApiKey(event.currentTarget.value)}
                    placeholder="napi_…"
                    required
                    type="password"
                    value={apiKey}
                  />
                </div>
                {error ? <ErrorText>{error}</ErrorText> : null}
              </DialogPanel>
              <DialogFooter>
                <DialogClose render={<Button variant="outline">Cancel</Button>} />
                <Button disabled={!organizationId} loading={save.isPending} type="submit">
                  {connected ? "Update API key" : "Connect Neon"}
                </Button>
              </DialogFooter>
            </form>
          </DialogPopup>
        </Dialog>
        {connected ? (
          <Button
            loading={disconnect.isPending}
            onClick={() => disconnect.mutate()}
            size="sm"
            variant="outline"
          >
            Disconnect
          </Button>
        ) : null}
      </div>
    </SectionCard>
  );
}

function DepotSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["depot-connection", organizationId];
  const [open, setOpen] = useState(false);
  const [token, setToken] = useState("");
  const [projectId, setProjectId] = useState("");
  const [orgId, setOrgId] = useState("");
  const [error, setError] = useState<string | null>(null);

  const connection = useQuery({
    queryKey,
    queryFn: getDepotConnection,
    enabled: Boolean(organizationId),
  });

  const save = useMutation({
    mutationFn: () => saveDepotConnection({ token, projectId, orgId }),
    onSuccess: async () => {
      setToken("");
      setProjectId("");
      setOrgId("");
      setError(null);
      setOpen(false);
      toast.success("Depot token saved");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  const disconnect = useMutation({
    mutationFn: disconnectDepot,
    onSuccess: async () => {
      toast.success("Depot disconnected");
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (error) => toast.error("Couldn't disconnect Depot", { description: toMessage(error) }),
  });

  const connected = connection.data?.connected;

  return (
    <SectionCard
      badge={<ConnectionBadge connected={connected} />}
      description="Connect a Depot project to build images for this workspace."
      icon={<ContainerIcon className="size-4.5" />}
      title="Depot"
    >
      {connected ? (
        <div className="mt-5 flex items-center gap-3 rounded-lg border bg-background/40 px-3 py-2.5">
          <StatusDot pulse tone="success" />
          <div className="min-w-0">
            <p className="truncate font-medium text-sm">
              project {connection.data?.projectId}
              <span className="text-muted-foreground"> · org {connection.data?.orgId ?? "—"}</span>
            </p>
            <MaskedValue hint={connection.data?.tokenHint} />
          </div>
        </div>
      ) : null}

      <div className="mt-4 flex flex-wrap gap-2">
        <Dialog
          open={open}
          onOpenChange={(next) => {
            setOpen(next);
            if (!next) setError(null);
          }}
        >
          <DialogTrigger
            render={
              <Button size="sm" variant={connected ? "outline" : "default"}>
                {connected ? "Update connection" : "Connect Depot"}
              </Button>
            }
          />
          <DialogPopup className="h-fit max-w-md">
            <form
              onSubmit={(event) => {
                event.preventDefault();
                save.mutate();
              }}
            >
              <DialogHeader>
                <DialogTitle>{connected ? "Update Depot connection" : "Connect Depot"}</DialogTitle>
                <DialogDescription>
                  Builds run on Depot and push to its registry before deploying to your servers.
                </DialogDescription>
              </DialogHeader>
              <DialogPanel className="space-y-4">
                <div className="space-y-2">
                  <Label htmlFor="depot-project">Project ID</Label>
                  <Input
                    autoFocus
                    id="depot-project"
                    onChange={(event) => setProjectId(event.currentTarget.value)}
                    placeholder="abc123def4"
                    required
                    value={projectId}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="depot-org">Organization ID</Label>
                  <Input
                    id="depot-org"
                    onChange={(event) => setOrgId(event.currentTarget.value)}
                    placeholder="the {orgId}.registry.depot.dev subdomain"
                    required
                    value={orgId}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="depot-token">Access token</Label>
                  <Input
                    autoComplete="off"
                    id="depot-token"
                    onChange={(event) => setToken(event.currentTarget.value)}
                    placeholder="depot_org_…"
                    required
                    type="password"
                    value={token}
                  />
                </div>
                {error ? <ErrorText>{error}</ErrorText> : null}
              </DialogPanel>
              <DialogFooter>
                <DialogClose render={<Button variant="outline">Cancel</Button>} />
                <Button disabled={!organizationId} loading={save.isPending} type="submit">
                  {connected ? "Update connection" : "Connect Depot"}
                </Button>
              </DialogFooter>
            </form>
          </DialogPopup>
        </Dialog>
        {connected ? (
          <Button
            loading={disconnect.isPending}
            onClick={() => disconnect.mutate()}
            size="sm"
            variant="outline"
          >
            Disconnect
          </Button>
        ) : null}
      </div>
    </SectionCard>
  );
}
