import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { FormEvent, useEffect, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
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
