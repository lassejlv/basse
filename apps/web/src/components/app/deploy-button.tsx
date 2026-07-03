import { useMutation, useQueryClient } from "@tanstack/react-query";
import { RocketIcon, RotateCcwIcon } from "lucide-react";
import { useState } from "react";
import type { Deployment } from "@basse/shared";
import { Button } from "@/components/ui/button";
import type { App } from "@/lib/apps";
import { triggerDeploy } from "@/lib/deployments";
import { toast } from "@/lib/toast";

type DeployMode = "default" | "latest-image" | "no-cache";

export function DeployButton({
  app,
  canDeploy,
  deployments,
  hasStagedChanges,
}: {
  app: App;
  canDeploy: boolean;
  deployments: Deployment[];
  hasStagedChanges: boolean;
}) {
  const queryClient = useQueryClient();
  const [error, setError] = useState<string | null>(null);
  const reusableImage = deployments.find(
    (deployment) =>
      deployment.imageRef && ["healthy", "crashed", "superseded"].includes(deployment.status),
  );
  const disabledTitle = hasStagedChanges
    ? "You have unsaved changes — deploy them from the bar below"
    : undefined;

  const deploy = useMutation({
    mutationFn: (mode: DeployMode) =>
      triggerDeploy(app.id, {
        useLatestImage: mode === "latest-image",
        noCache: mode === "no-cache",
      }),
    onSuccess: async (_deployment, mode) => {
      setError(null);
      toast.success(
        mode === "latest-image"
          ? "Redeploy queued"
          : mode === "no-cache"
            ? "No-cache deployment queued"
            : "Deployment queued",
      );
      await queryClient.invalidateQueries({ queryKey: ["deployments", app.id] });
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <div className="flex flex-col gap-2">
      {/* While changes are staged, deploying goes through the staged-changes bar
          (apply + deploy); this button would otherwise ship the live config and
          silently skip the staged edits. */}
      <div className="flex flex-wrap gap-2">
        <Button
          disabled={!canDeploy || hasStagedChanges || deploy.isPending}
          loading={deploy.isPending && deploy.variables === "default"}
          onClick={() => deploy.mutate("default")}
          size="sm"
          title={disabledTitle}
        >
          <RocketIcon />
          Deploy
        </Button>
        {app.sourceType === "repository" ? (
          <>
            <Button
              disabled={!canDeploy || hasStagedChanges || deploy.isPending || !reusableImage}
              loading={deploy.isPending && deploy.variables === "latest-image"}
              onClick={() => deploy.mutate("latest-image")}
              size="sm"
              title={
                disabledTitle ??
                (!reusableImage ? "No successful image to redeploy yet" : undefined)
              }
              variant="outline"
            >
              <RotateCcwIcon />
              Skip build
            </Button>
            <Button
              disabled={!canDeploy || hasStagedChanges || deploy.isPending}
              loading={deploy.isPending && deploy.variables === "no-cache"}
              onClick={() => deploy.mutate("no-cache")}
              size="sm"
              title={disabledTitle}
              variant="outline"
            >
              <RocketIcon />
              No cache
            </Button>
          </>
        ) : null}
      </div>
      {hasStagedChanges ? (
        <p className="text-muted-foreground text-xs">Deploy staged changes from the bar below.</p>
      ) : error ? (
        <p className="text-destructive-foreground text-xs">{error}</p>
      ) : null}
    </div>
  );
}
