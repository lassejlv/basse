import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { CheckIcon, CopyIcon, ExternalLinkIcon, TrashIcon } from "lucide-react";
import { FormEvent, useEffect, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { Textarea } from "@/components/ui/textarea";
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
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Secrets</h1>
        <p className="mt-2 text-muted-foreground text-sm">
          SSH keys and integration credentials for {activeOrganization?.name ?? "this workspace"}.
        </p>
      </div>

      <SshKeysSection organizationId={organizationId} />
      <GitHubSection organizationId={organizationId} />
      <DepotSection organizationId={organizationId} />
    </section>
  );
}

function GitHubSection({ organizationId }: { organizationId?: string }) {
  const queryClient = useQueryClient();
  const navigate = useNavigate();
  const search = Route.useSearch();
  const processedCode = useRef<string | null>(null);
  const processedInstallation = useRef<string | null>(null);
  const [copiedWebhook, setCopiedWebhook] = useState(false);
  const [startingGitHubSetup, setStartingGitHubSetup] = useState(false);
  const integrationKey = ["github-app-integration", organizationId];
  const installationsKey = ["github-app-installations", organizationId];

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
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: integrationKey }),
        queryClient.invalidateQueries({ queryKey: installationsKey }),
        queryClient.invalidateQueries({ queryKey: ["github-repositories"] }),
      ]);
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
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: installationsKey }),
        queryClient.invalidateQueries({ queryKey: ["github-repositories"] }),
      ]);
      await clearGitHubCallbackSearch(navigate);
    },
    onError: (error) =>
      toast.error("Couldn't save GitHub installation", { description: toMessage(error) }),
  });

  const disconnect = useMutation({
    mutationFn: disconnectGitHubApp,
    onSuccess: async () => {
      toast.success("GitHub disconnected");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: integrationKey }),
        queryClient.invalidateQueries({ queryKey: installationsKey }),
        queryClient.invalidateQueries({ queryKey: ["github-repositories"] }),
      ]);
    },
    onError: (error) =>
      toast.error("Couldn't disconnect GitHub", { description: toMessage(error) }),
  });

  const removeInstallation = useMutation({
    mutationFn: deleteGitHubAppInstallation,
    onSuccess: async () => {
      toast.success("GitHub installation removed");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: installationsKey }),
        queryClient.invalidateQueries({ queryKey: ["github-repositories"] }),
      ]);
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

  const connected = integration.data?.connected;
  const installUrl = integration.data?.installUrl;
  const webhookUrl = integration.data?.webhookUrl ?? manifest.data?.webhookUrl;
  const savedInstallations = installations.data ?? [];
  const installCallbackMissingId =
    Boolean(search.setup_action) && !search.installation_id && !search.code;

  async function copyWebhookUrl() {
    if (!webhookUrl) return;
    await navigator.clipboard.writeText(webhookUrl);
    setCopiedWebhook(true);
    window.setTimeout(() => setCopiedWebhook(false), 1500);
  }

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
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">GitHub</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Create a workspace GitHub App, install it on private repositories, and deploy over
        short-lived installation tokens.
      </p>

      {integration.isError ? (
        <p className="mt-4 text-destructive-foreground text-sm">
          Couldn't load GitHub integration: {toMessage(integration.error)}
        </p>
      ) : null}
      {manifest.isError ? (
        <p className="mt-4 text-destructive-foreground text-sm">
          Couldn't prepare GitHub App setup: {toMessage(manifest.error)}
        </p>
      ) : null}
      {installations.isError ? (
        <p className="mt-4 text-destructive-foreground text-sm">
          Couldn't load GitHub installations: {toMessage(installations.error)}
        </p>
      ) : null}
      {installCallbackMissingId ? (
        <p className="mt-4 text-muted-foreground text-sm">
          GitHub returned from setup without an installation id. Choose an account and repository
          access on GitHub, then complete the installation.
        </p>
      ) : null}

      {connected ? (
        <div className="mt-5 rounded-md border px-3 py-2 text-sm">
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
              onClick={copyWebhookUrl}
              size="icon"
              type="button"
              variant="outline"
            >
              {copiedWebhook ? <CheckIcon /> : <CopyIcon />}
            </Button>
          </div>
        </div>
      ) : null}

      {savedInstallations.length > 0 ? (
        <ul className="mt-4 flex flex-col gap-2">
          {savedInstallations.map((installation) => (
            <li
              key={installation.id}
              className="flex items-center justify-between gap-3 rounded-md border px-3 py-2"
            >
              <div className="min-w-0 text-sm">
                <p className="truncate font-medium">{installation.accountLogin}</p>
                <p className="truncate text-muted-foreground text-xs">
                  {installation.accountType ?? "account"} ·{" "}
                  {installation.repositorySelection ?? "repositories"}
                </p>
              </div>
              <Button
                aria-label={`Remove ${installation.accountLogin} installation`}
                loading={removeInstallation.isPending}
                onClick={() => {
                  if (
                    window.confirm(
                      `Remove ${installation.accountLogin} from this workspace's GitHub installations?`,
                    )
                  ) {
                    removeInstallation.mutate(installation.id);
                  }
                }}
                size="icon"
                type="button"
                variant="outline"
              >
                <TrashIcon />
              </Button>
            </li>
          ))}
        </ul>
      ) : connected ? (
        <p className="mt-4 text-muted-foreground text-sm">No GitHub installations saved yet.</p>
      ) : null}

      <div className="mt-6 flex flex-wrap gap-2 border-t pt-6">
        <form action={manifest.data?.actionUrl} method="post" onSubmit={submitGitHubManifest}>
          <input name="manifest" type="hidden" value={manifest.data?.manifest ?? ""} />
          <Button
            disabled={!organizationId || manifest.isPending || startingGitHubSetup}
            loading={startingGitHubSetup}
            type="submit"
          >
            {connected ? "Replace GitHub App" : "Create GitHub App"}
          </Button>
        </form>

        {installUrl ? (
          <>
            <Button render={<a href={installUrl} />} variant="outline">
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
              variant="outline"
            >
              Disconnect
            </Button>
          </>
        ) : null}
      </div>
    </div>
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

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    addKey.mutate();
  }

  const keyList = keys.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">SSH Keys</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Private keys Basse can use when connecting to servers in this workspace.
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
                    {key.publicKey} {key.hasPrivateKey ? "" : " · public key only"}
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
          <Label htmlFor="ssh-key-private">Private key</Label>
          <Textarea
            id="ssh-key-private"
            value={privateKey}
            onChange={(event) => setPrivateKey(event.currentTarget.value)}
            placeholder="-----BEGIN OPENSSH PRIVATE KEY-----"
            rows={6}
            className="font-mono text-xs"
            required
          />
          <p className="text-muted-foreground text-xs">
            Basse stores it encrypted and derives the public key automatically.
          </p>
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
              project {connection.data?.projectId} · org {connection.data?.orgId ?? "—"} · token
              ••••
              {connection.data?.tokenHint}
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
          <Label htmlFor="depot-org">Organization ID</Label>
          <Input
            id="depot-org"
            value={orgId}
            onChange={(event) => setOrgId(event.currentTarget.value)}
            placeholder="the {orgId}.registry.depot.dev subdomain"
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
