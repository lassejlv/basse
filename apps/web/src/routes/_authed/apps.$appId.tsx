import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { useState } from "react";
import { Button } from "@/components/ui/button";
import { Textarea } from "@/components/ui/textarea";
import { getApp } from "@/lib/apps";
import { listEnvVars, setEnvVars } from "@/lib/env-vars";

export const Route = createFileRoute("/_authed/apps/$appId")({
  component: AppDetailRoute,
});

function AppDetailRoute() {
  const { appId } = Route.useParams();
  const app = useQuery({ queryKey: ["app", appId], queryFn: () => getApp(appId) });

  if (app.isPending) {
    return <p className="p-6 text-muted-foreground text-sm">Loading…</p>;
  }
  if (app.isError || !app.data) {
    return <p className="p-6 text-destructive-foreground text-sm">App not found.</p>;
  }

  const data = app.data;

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">{data.name}</h1>
        <p className="mt-2 font-mono text-muted-foreground text-sm">
          {data.repositoryUrl} · {data.branch} · :{data.port} · {data.buildMode}
        </p>
        {!data.serverId ? (
          <p className="mt-2 text-warning-foreground text-sm">
            No server attached — set one before deploying.
          </p>
        ) : null}
      </div>

      <EnvVarsCard appId={appId} />
    </section>
  );
}

function EnvVarsCard({ appId }: { appId: string }) {
  const queryClient = useQueryClient();
  const queryKey = ["env-vars", appId];
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);

  const vars = useQuery({ queryKey, queryFn: () => listEnvVars(appId) });

  const save = useMutation({
    mutationFn: () => {
      const parsed = draft
        .split("\n")
        .map((line) => line.trim())
        .filter((line) => line && !line.startsWith("#"))
        .map((line) => {
          const eq = line.indexOf("=");
          return { key: line.slice(0, eq).trim(), value: line.slice(eq + 1) };
        })
        .filter((v) => v.key);
      return setEnvVars(appId, parsed);
    },
    onSuccess: async () => {
      setDraft("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  const list = vars.data ?? [];

  return (
    <div className="max-w-2xl rounded-lg border bg-card p-6">
      <h2 className="text-lg font-semibold">Environment variables</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Runtime variables, encrypted at rest. Changes apply on the next deploy.
      </p>

      <div className="mt-5">
        {vars.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : list.length === 0 ? (
          <p className="text-muted-foreground text-sm">No variables set.</p>
        ) : (
          <ul className="flex flex-col gap-1">
            {list.map((v) => (
              <li key={v.key} className="flex justify-between gap-3 font-mono text-sm">
                <span className="font-medium">{v.key}</span>
                <span className="text-muted-foreground">{v.valueHint}</span>
              </li>
            ))}
          </ul>
        )}
      </div>

      <div className="mt-6 space-y-3 border-t pt-6">
        <p className="text-sm">
          Paste <code className="font-mono">KEY=value</code> lines to <strong>replace</strong> the
          whole set:
        </p>
        <Textarea
          className="font-mono text-xs"
          onChange={(e) => setDraft(e.currentTarget.value)}
          placeholder={"DATABASE_URL=postgres://…\nPORT=3000"}
          rows={5}
          value={draft}
        />
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button disabled={!draft.trim()} loading={save.isPending} onClick={() => save.mutate()}>
          Save variables
        </Button>
      </div>
    </div>
  );
}
