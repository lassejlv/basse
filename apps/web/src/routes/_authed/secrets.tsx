import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  CheckIcon,
  ContainerIcon,
  CopyIcon,
  ExternalLinkIcon,
  GitBranchIcon,
  KeyRoundIcon,
  PlusIcon,
} from "lucide-react";
import { FormEvent, type ReactNode, useEffect, useRef, useState } from "react";
import type { ApiTokenScope } from "@basse/shared";
import { useClipboard } from "@/components/app/shared";
import {
  EmptyNote,
  ErrorText,
  Row,
  RowDeleteButton,
  RowList,
  SectionLabel,
} from "@/components/dashboard";
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

function SecretsRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;

  return (
    <section className="flex flex-1 flex-col gap-7 p-4 md:p-6">
      <div>
        <SectionLabel>Workspace</SectionLabel>
        <h1 className="mt-1 font-semibold text-2xl tracking-tight md:text-3xl">Secrets</h1>
        <p className="mt-1 text-muted-foreground text-sm">
          SSH keys and integration credentials for {activeOrganization?.name ?? "this workspace"}.
        </p>
      </div>

      <div className="grid max-w-5xl items-start gap-4 xl:grid-cols-2">
        <GitHubSection organizationId={organizationId} />
        <div className="flex flex-col gap-4">
          <ApiTokensSection organizationId={organizationId} />
          <SshKeysSection organizationId={organizationId} />
          <DepotSection organizationId={organizationId} />
        </div>
      </div>
    </section>
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
          <RowList>
            {tokenList.map((token) => (
              <Row
                action={
                  <RowDeleteButton
                    label={`Delete ${token.name}`}
                    loading={remove.isPending && remove.variables === token.id}
                    onClick={() => remove.mutate(token.id)}
                  />
                }
                key={token.id}
              >
                <p className="truncate font-medium text-sm">{token.name}</p>
                <p className="truncate text-muted-foreground text-xs">
                  {token.tokenPrefix} · {token.scopes.join(", ")} ·{" "}
                  {token.lastUsedAt
                    ? `last used ${new Date(token.lastUsedAt).toLocaleString()}`
                    : "never used"}
                </p>
              </Row>
            ))}
          </RowList>
        ) : null}
        {tokens.isSuccess && tokenList.length === 0 ? (
          <EmptyNote>No API tokens yet.</EmptyNote>
        ) : null}
        {tokens.isError ? (
          <ErrorText>Couldn't load API tokens: {toMessage(tokens.error)}</ErrorText>
        ) : null}
      </div>

      <Dialog open={open} onOpenChange={setOpen}>
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
                <Label htmlFor="created-api-token">Token</Label>
                <Textarea id="created-api-token" readOnly value={createdToken} />
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
                        className="flex items-start gap-2 rounded-md border px-3 py-2 text-sm"
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
        <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border bg-muted/30 text-foreground/80">
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
        <div className="mt-5 rounded-lg border px-3 py-2.5 text-sm">
          <p className="font-medium">{integration.data?.appName}</p>
          <p className="font-mono text-muted-foreground text-xs">
            github.com/apps/{integration.data?.appSlug}
          </p>
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
          <RowList>
            {savedInstallations.map((installation) => (
              <Row
                action={
                  <RowDeleteButton
                    confirmMessage={`Remove ${installation.accountLogin} from this workspace's GitHub installations?`}
                    label={`Remove ${installation.accountLogin} installation`}
                    loading={removeInstallation.isPending}
                    onClick={() => removeInstallation.mutate(installation.id)}
                  />
                }
                key={installation.id}
              >
                <p className="truncate font-medium text-sm">{installation.accountLogin}</p>
                <p className="truncate text-muted-foreground text-xs">
                  {installation.accountType ?? "account"} ·{" "}
                  {installation.repositorySelection ?? "repositories"}
                </p>
              </Row>
            ))}
          </RowList>
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
      icon={<KeyRoundIcon className="size-4.5" />}
      title="SSH keys"
    >
      <div className="mt-5">
        {keys.isPending ? (
          <div className="h-16 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
        ) : keyList.length === 0 ? (
          <EmptyNote>No SSH keys yet. Add one to connect servers with an existing key.</EmptyNote>
        ) : (
          <RowList>
            {keyList.map((key) => (
              <Row
                action={
                  <RowDeleteButton
                    label={`Delete ${key.name}`}
                    loading={removeKey.isPending && removeKey.variables === key.id}
                    onClick={() => removeKey.mutate(key.id)}
                  />
                }
                key={key.id}
              >
                <p className="truncate font-medium text-sm">{key.name}</p>
                <p className="truncate font-mono text-muted-foreground text-xs">
                  {key.publicKey}
                  {key.hasPrivateKey ? "" : " · public key only"}
                </p>
              </Row>
            ))}
          </RowList>
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
        <div className="mt-5 rounded-lg border px-3 py-2.5">
          <p className="font-medium text-sm">Connected</p>
          <p className="truncate font-mono text-muted-foreground text-xs">
            project {connection.data?.projectId} · org {connection.data?.orgId ?? "—"} · token ••••
            {connection.data?.tokenHint}
          </p>
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
