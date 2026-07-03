import { useQuery } from "@tanstack/react-query";
import type { App } from "@/lib/apps";
import { getApp } from "@/lib/apps";
import {
  getChangeHistory,
  getChanges,
  getProjectChangeHistory,
  getProjectChanges,
} from "@/lib/changes";
import { listDeployments } from "@/lib/deployments";

/**
 * Everything the app panel needs: live app, deployments (refreshed via the
 * realtime socket), and staged changes (app- and project-scoped). `draft` is the
 * live app with staged config overlaid — settings forms seed from it, while
 * the header and deployments show live state.
 */
export function useAppDetail(appId: string) {
  const app = useQuery({ queryKey: ["app", appId], queryFn: () => getApp(appId) });

  // Status and log updates arrive over the realtime socket (lib/realtime.ts).
  const deployments = useQuery({
    queryKey: ["deployments", appId],
    queryFn: () => listDeployments(appId),
  });

  const changes = useQuery({ queryKey: ["changes", appId], queryFn: () => getChanges(appId) });
  const changeHistory = useQuery({
    queryKey: ["change-history", appId],
    queryFn: () => getChangeHistory(appId),
  });
  const projectChanges = useQuery({
    queryKey: ["project-changes", app.data?.projectId],
    queryFn: () => getProjectChanges(app.data!.projectId!),
    enabled: Boolean(app.data?.projectId),
  });
  const projectChangeHistory = useQuery({
    queryKey: ["project-change-history", app.data?.projectId],
    queryFn: () => getProjectChangeHistory(app.data!.projectId!),
    enabled: Boolean(app.data?.projectId),
  });

  const data: App | undefined = app.data;
  const draft = changes.data?.draft ?? data;
  const stagedChanges = changes.data?.changes ?? [];
  const projectStagedChanges = projectChanges.data?.changes ?? [];
  const currentAppStagedChanges = data?.projectId
    ? projectStagedChanges.filter((change) => change.appId === data.id)
    : stagedChanges;
  const hasStagedChanges = data?.projectId
    ? (projectChanges.data?.changes.length ?? stagedChanges.length) > 0
    : stagedChanges.length > 0;
  const deploymentList = deployments.data ?? [];
  const status = deploymentList[0]?.status ?? data?.latestDeploymentStatus ?? null;
  const canDeploy = data
    ? data.appKind === "neon"
      ? false
      : data.appKind === "database"
        ? data.serverIds.length === 1
        : data.serverIds.length > 0 &&
          (data.sourceType === "image" ||
            data.buildRunner !== "server" ||
            data.serverIds.length === 1)
    : false;

  return {
    appQuery: app,
    deploymentsQuery: deployments,
    changeHistoryQuery: changeHistory,
    projectChangeHistoryQuery: projectChangeHistory,
    data,
    draft,
    stagedChanges,
    projectStagedChanges,
    currentAppStagedChanges,
    hasStagedChanges,
    deploymentList,
    status,
    canDeploy,
  };
}

export type AppDetail = ReturnType<typeof useAppDetail>;
