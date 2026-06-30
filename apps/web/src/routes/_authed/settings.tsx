import { useMutation } from "@tanstack/react-query";
import { createFileRoute, useNavigate } from "@tanstack/react-router";
import { Button } from "@/components/ui/button";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/_authed/settings")({
  component: SettingsRoute,
});

function SettingsRoute() {
  const navigate = useNavigate();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const { data: session } = authClient.useSession();

  const signOut = useMutation({
    mutationFn: () => authClient.signOut(),
    onSuccess: () => navigate({ to: "/login" }),
  });

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
