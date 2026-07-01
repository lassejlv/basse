import { ExternalLinkIcon, Maximize2Icon, Minimize2Icon, XIcon } from "lucide-react";
import { useState } from "react";
import { DatabaseIcon, databaseEngineLabel } from "@/components/database-icon";
import { DeployStatusBadge, StatusDot } from "@/components/deploy-status";
import { ProjectStagedChangesHistory, StagedChangesHistory } from "@/components/staged-changes-bar";
import { Button } from "@/components/ui/button";
import { Tabs, TabsList, TabsPanel, TabsTab } from "@/components/ui/tabs";
import type { App } from "@/lib/apps";
import { cn } from "@/lib/utils";
import { BackupsTab } from "./backups-tab";
import { DatabaseConnectionCard } from "./connection-tab";
import { DeployButton } from "./deploy-button";
import { DeploymentsPanel } from "./deployments-tab";
import { AppDomainsTab } from "./domains-tab";
import { AppSettingsTab } from "./settings-tab";
import { SpecDivider, useLiveUrl } from "./shared";
import { useAppDetail } from "./use-app-detail";
import { EnvVarsCard } from "./variables-tab";

const SERVICE_TABS = ["deployments", "variables", "domains", "changes", "settings"];
const DATABASE_TABS = ["deployments", "connection", "backups", "changes", "settings"];

/** The app sidecard on the project canvas — the entire app experience lives
 * here: deployments, variables, domains, staged-change history, settings. */
export function AppPanel({
  appId,
  tab,
  onTabChange,
  onClose,
}: {
  appId: string;
  tab?: string;
  onTabChange: (tab: string) => void;
  onClose: () => void;
}) {
  const [maximized, setMaximized] = useState(false);
  const detail = useAppDetail(appId);
  const app = detail.data;

  const validTabs = app?.appKind === "database" ? DATABASE_TABS : SERVICE_TABS;
  const activeTab = tab && validTabs.includes(tab) ? tab : "deployments";

  return (
    <aside
      aria-label={app ? `${app.name} details` : "App details"}
      className={cn(
        "fade-in-0 slide-in-from-right-4 absolute inset-y-3 right-3 z-20 flex animate-in flex-col overflow-hidden rounded-xl border bg-card shadow-xl duration-200 motion-reduce:animate-none",
        maximized ? "left-3" : "w-[min(30rem,calc(100%-1.5rem))]",
      )}
    >
      {detail.appQuery.isPending ? (
        <div className="flex flex-1 flex-col gap-3 p-4">
          <div className="h-6 w-1/2 animate-pulse rounded bg-muted/50" aria-hidden />
          <div className="h-4 w-2/3 animate-pulse rounded bg-muted/40" aria-hidden />
          <div className="mt-4 flex-1 animate-pulse rounded-lg bg-muted/20" aria-hidden />
        </div>
      ) : detail.appQuery.isError || !app ? (
        <div className="flex flex-1 flex-col items-center justify-center gap-3 p-6">
          <p className="text-muted-foreground text-sm">This app no longer exists.</p>
          <Button onClick={onClose} size="sm" variant="outline">
            Close
          </Button>
        </div>
      ) : (
        <>
          <AppPanelHeader
            app={app}
            maximized={maximized}
            onClose={onClose}
            onToggleMaximize={() => setMaximized((value) => !value)}
            status={detail.status}
          />
          <div className="border-b px-4 pb-3">
            <DeployButton
              app={app}
              canDeploy={detail.canDeploy}
              deployments={detail.deploymentList}
              hasStagedChanges={detail.hasStagedChanges}
            />
            {app.serverIds.length === 0 ? (
              <p className="mt-2 text-warning-foreground text-xs">
                No servers attached — select at least one in Settings before deploying.
              </p>
            ) : null}
          </div>
          <Tabs
            className="flex min-h-0 flex-1 flex-col gap-0"
            onValueChange={(value) => onTabChange(value as string)}
            value={activeTab}
          >
            <TabsList
              className="w-full shrink-0 justify-start overflow-x-auto rounded-none border-b px-4"
              variant="underline"
            >
              <TabsTab value="deployments">Deployments</TabsTab>
              {app.appKind === "database" ? (
                <>
                  <TabsTab value="connection">Connection</TabsTab>
                  {app.database?.kind === "postgres" ? (
                    <TabsTab value="backups">Backups</TabsTab>
                  ) : null}
                </>
              ) : (
                <>
                  <TabsTab value="variables">Variables</TabsTab>
                  <TabsTab value="domains">Domains</TabsTab>
                </>
              )}
              <TabsTab value="changes">Changes</TabsTab>
              <TabsTab value="settings">Settings</TabsTab>
            </TabsList>
            <div className="min-h-0 flex-1 overflow-y-auto p-4">
              <TabsPanel value="deployments">
                <DeploymentsPanel
                  app={app}
                  deployments={detail.deploymentList}
                  isPending={detail.deploymentsQuery.isPending}
                />
              </TabsPanel>
              {app.appKind === "database" ? (
                <>
                  <TabsPanel value="connection">
                    <DatabaseConnectionCard app={app} />
                  </TabsPanel>
                  {app.database?.kind === "postgres" ? (
                    <TabsPanel value="backups">
                      <BackupsTab app={app} />
                    </TabsPanel>
                  ) : null}
                </>
              ) : (
                <>
                  <TabsPanel value="variables">
                    <EnvVarsCard appId={appId} stagedChanges={detail.stagedChanges} />
                  </TabsPanel>
                  <TabsPanel value="domains">
                    <AppDomainsTab
                      app={app}
                      draft={detail.draft ?? app}
                      projectId={app.projectId}
                      stagedChanges={detail.currentAppStagedChanges}
                    />
                  </TabsPanel>
                </>
              )}
              <TabsPanel value="changes">
                {app.projectId ? (
                  <ProjectStagedChangesHistory
                    entries={detail.projectChangeHistoryQuery.data ?? []}
                    isPending={detail.projectChangeHistoryQuery.isPending}
                  />
                ) : (
                  <StagedChangesHistory
                    entries={detail.changeHistoryQuery.data ?? []}
                    isPending={detail.changeHistoryQuery.isPending}
                  />
                )}
              </TabsPanel>
              <TabsPanel value="settings">
                <AppSettingsTab app={app} draft={detail.draft ?? app} />
              </TabsPanel>
            </div>
          </Tabs>
        </>
      )}
    </aside>
  );
}

function AppPanelHeader({
  app,
  status,
  maximized,
  onToggleMaximize,
  onClose,
}: {
  app: App;
  status: App["latestDeploymentStatus"];
  maximized: boolean;
  onToggleMaximize: () => void;
  onClose: () => void;
}) {
  const liveUrl = useLiveUrl(app);
  const database = app.database;
  const repoHost = app.repositoryUrl.replace(/^https?:\/\//, "");

  return (
    <div className="flex items-start gap-3 p-4 pb-3">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <StatusDot status={status} />
          <h2 className="truncate font-semibold text-base">{app.name}</h2>
          <DeployStatusBadge size="sm" status={status} />
        </div>
        <div className="mt-1.5 flex flex-wrap items-center gap-x-2 gap-y-0.5 font-mono text-muted-foreground text-xs">
          {database ? (
            <>
              <span className="inline-flex items-center gap-1.5">
                <DatabaseIcon className="size-3.5" kind={database.kind} />
                {databaseEngineLabel(database.kind)} {database.version}
              </span>
              <SpecDivider />
              <span className="truncate">
                {database.internalHost}:{database.internalPort}
              </span>
            </>
          ) : (
            <>
              <span className="truncate">
                {app.sourceType === "image" ? app.imageRef : repoHost}
              </span>
              {app.sourceType === "repository" ? (
                <>
                  <SpecDivider />
                  <span>{app.branch}</span>
                </>
              ) : null}
              <SpecDivider />
              <span>:{app.port}</span>
            </>
          )}
        </div>
        {liveUrl ? (
          <a
            className="mt-1 inline-flex max-w-full items-center gap-1 truncate font-medium text-foreground text-xs underline-offset-4 hover:underline"
            href={liveUrl}
            rel="noreferrer"
            target="_blank"
          >
            {liveUrl.replace(/^https?:\/\//, "")}
            <ExternalLinkIcon className="size-3 shrink-0 opacity-70" />
          </a>
        ) : null}
      </div>
      <div className="flex shrink-0 items-center gap-1">
        <Button
          aria-label={maximized ? "Restore panel" : "Maximize panel"}
          onClick={onToggleMaximize}
          size="icon-sm"
          variant="ghost"
        >
          {maximized ? <Minimize2Icon /> : <Maximize2Icon />}
        </Button>
        <Button aria-label="Close panel" onClick={onClose} size="icon-sm" variant="ghost">
          <XIcon />
        </Button>
      </div>
    </div>
  );
}
