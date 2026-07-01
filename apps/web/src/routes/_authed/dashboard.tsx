import { useQuery } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import {
  ArrowUpRightIcon,
  BellIcon,
  BoxIcon,
  ChevronRightIcon,
  FolderIcon,
  ServerIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { ServerStatusBadge } from "@/components/server-status-badge";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { listAlerts } from "@/lib/alerts";
import { authClient } from "@/lib/auth-client";
import { maskHost, relativeTime } from "@/lib/format";
import { listProjects } from "@/lib/projects";
import { listServers } from "@/lib/servers";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_authed/dashboard")({
  component: DashboardRoute,
});

function DashboardRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;
  const enabled = Boolean(organizationId);

  const projects = useQuery({
    queryKey: ["projects", organizationId],
    queryFn: listProjects,
    enabled,
  });
  const servers = useQuery({
    queryKey: ["servers", organizationId],
    queryFn: listServers,
    enabled,
    refetchInterval: 30_000,
  });
  const alerts = useQuery({
    queryKey: ["alerts", "active", organizationId],
    queryFn: () => listAlerts("active"),
    enabled,
    refetchInterval: 30_000,
  });

  const projectList = projects.data ?? [];
  const serverList = servers.data ?? [];
  const alertList = alerts.data ?? [];
  const activeServers = serverList.filter((server) => server.status === "active");
  const appTotal = projectList.reduce((sum, project) => sum + project.appCount, 0);

  return (
    <section className="flex flex-1 flex-col gap-7 p-4 md:p-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
            Overview
          </p>
          <h1 className="mt-1 font-semibold text-2xl tracking-tight md:text-3xl">
            {activeOrganization?.name ?? "Workspace"}
          </h1>
        </div>
        <Button render={<Link to="/projects" />} variant="outline">
          <FolderIcon />
          All projects
        </Button>
      </div>

      <Card className="grid grid-cols-2 gap-0 divide-y p-0 sm:grid-cols-4 sm:divide-x sm:divide-y-0">
        <StatTile
          label="Projects"
          pending={projects.isPending}
          value={String(projectList.length)}
        />
        <StatTile label="Apps" pending={projects.isPending} value={String(appTotal)} />
        <StatTile
          label="Servers online"
          pending={servers.isPending}
          tone={
            serverList.length > 0 && activeServers.length < serverList.length ? "warn" : undefined
          }
          value={serverList.length === 0 ? "0" : `${activeServers.length}/${serverList.length}`}
        />
        <StatTile
          label="Active alerts"
          pending={alerts.isPending}
          tone={alertList.length > 0 ? "alert" : undefined}
          value={String(alertList.length)}
        />
      </Card>

      <div className="grid gap-6 xl:grid-cols-[2fr_1fr]">
        <section className="min-w-0">
          <SectionHeading
            action={
              <Link
                className="inline-flex items-center gap-1 text-muted-foreground text-xs transition hover:text-foreground"
                to="/projects"
              >
                View all
                <ArrowUpRightIcon className="size-3" />
              </Link>
            }
            title="Projects"
          />
          {projects.isPending ? (
            <div className="grid gap-3 sm:grid-cols-2">
              <div className="h-28 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
              <div className="h-28 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
            </div>
          ) : projectList.length === 0 ? (
            <EmptyHint
              action={
                <Button render={<Link to="/projects" />} size="sm" variant="outline">
                  Create a project
                </Button>
              }
              icon={<FolderIcon className="size-5" />}
              text="No projects yet. A project groups environments and apps on the canvas."
            />
          ) : (
            <div className="grid gap-3 sm:grid-cols-2">
              {projectList.map((project) => (
                <Link
                  className="group rounded-xl border bg-card p-4 shadow-sm transition-[border-color] hover:border-muted-foreground/40"
                  key={project.id}
                  params={{ projectId: project.id }}
                  search={{ app: undefined, tab: undefined }}
                  to="/projects/$projectId"
                >
                  <div className="flex items-center justify-between gap-2">
                    <p className="min-w-0 truncate font-medium">{project.name}</p>
                    <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground/50 transition group-hover:translate-x-0.5 group-hover:text-muted-foreground" />
                  </div>
                  <p className="mt-0.5 truncate font-mono text-muted-foreground text-xs">
                    {project.slug}
                  </p>
                  <div className="mt-3 flex items-center gap-2">
                    <Badge size="sm" variant="outline">
                      {project.environmentCount} env{project.environmentCount === 1 ? "" : "s"}
                    </Badge>
                    <Badge size="sm" variant="outline">
                      <BoxIcon className="size-3" />
                      {project.appCount} app{project.appCount === 1 ? "" : "s"}
                    </Badge>
                    <span className="ml-auto text-muted-foreground text-xs">
                      {relativeTime(project.createdAt)}
                    </span>
                  </div>
                </Link>
              ))}
            </div>
          )}
        </section>

        <div className="flex min-w-0 flex-col gap-6">
          <section>
            <SectionHeading
              action={
                <Link
                  className="inline-flex items-center gap-1 text-muted-foreground text-xs transition hover:text-foreground"
                  to="/servers"
                >
                  Manage
                  <ArrowUpRightIcon className="size-3" />
                </Link>
              }
              title="Servers"
            />
            {servers.isPending ? (
              <div className="h-24 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
            ) : serverList.length === 0 ? (
              <EmptyHint
                action={
                  <Button render={<Link to="/servers" />} size="sm" variant="outline">
                    Connect a server
                  </Button>
                }
                icon={<ServerIcon className="size-5" />}
                text="No servers connected. Apps deploy onto your own machines."
              />
            ) : (
              <Card className="divide-y overflow-hidden p-0">
                {serverList.map((server) => (
                  <Link
                    className="flex items-center gap-3 px-4 py-3 transition hover:bg-accent/40"
                    key={server.id}
                    params={{ serverId: server.id }}
                    to="/servers/$serverId"
                  >
                    <div className="min-w-0 flex-1">
                      <p className="truncate font-medium text-sm">{server.name}</p>
                      <p className="truncate font-mono text-muted-foreground text-xs">
                        {maskHost(server.sshHost)}
                        {server.lastSeenAt ? ` · seen ${relativeTime(server.lastSeenAt)}` : ""}
                      </p>
                    </div>
                    <ServerStatusBadge status={server.status} />
                  </Link>
                ))}
              </Card>
            )}
          </section>

          <section>
            <SectionHeading
              action={
                <Link
                  className="inline-flex items-center gap-1 text-muted-foreground text-xs transition hover:text-foreground"
                  to="/alerts"
                >
                  View all
                  <ArrowUpRightIcon className="size-3" />
                </Link>
              }
              title="Alerts"
            />
            {alerts.isPending ? (
              <div className="h-16 animate-pulse rounded-xl border bg-muted/30" aria-hidden />
            ) : alertList.length === 0 ? (
              <EmptyHint
                icon={<BellIcon className="size-5" />}
                text="No active alerts. Monitors report here when something needs attention."
              />
            ) : (
              <Card className="divide-y overflow-hidden p-0">
                {alertList.slice(0, 5).map((alert) => (
                  <Link
                    className="flex gap-3 px-4 py-3 transition hover:bg-accent/40"
                    key={alert.id}
                    to="/alerts"
                  >
                    <span
                      className={cn(
                        "mt-1.5 size-2 shrink-0 rounded-full",
                        alert.severity === "critical"
                          ? "bg-destructive"
                          : alert.severity === "warning"
                            ? "bg-warning"
                            : "bg-info",
                      )}
                    />
                    <span className="min-w-0 flex-1">
                      <span className="block truncate font-medium text-sm">{alert.title}</span>
                      <span className="block truncate text-muted-foreground text-xs">
                        {alert.serverName ?? alert.appName ?? alert.code} ·{" "}
                        {relativeTime(alert.lastSeenAt)}
                      </span>
                    </span>
                  </Link>
                ))}
              </Card>
            )}
          </section>
        </div>
      </div>
    </section>
  );
}

function SectionHeading({ title, action }: { title: string; action?: ReactNode }) {
  return (
    <div className="mb-3 flex items-center justify-between gap-3">
      <h2 className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
        {title}
      </h2>
      {action}
    </div>
  );
}

function StatTile({
  label,
  value,
  pending,
  tone,
}: {
  label: string;
  value: string;
  pending: boolean;
  tone?: "warn" | "alert";
}) {
  return (
    <div className="p-5">
      <p className="font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]">
        {label}
      </p>
      {pending ? (
        <div className="mt-2 h-7 w-12 animate-pulse rounded bg-muted/50" aria-hidden />
      ) : (
        <p
          className={cn(
            "mt-1 font-mono font-semibold text-2xl tabular-nums",
            tone === "alert" && "text-destructive-foreground",
            tone === "warn" && "text-warning-foreground",
          )}
        >
          {value}
        </p>
      )}
    </div>
  );
}

function EmptyHint({ icon, text, action }: { icon: ReactNode; text: string; action?: ReactNode }) {
  return (
    <div className="flex flex-col items-start gap-3 rounded-xl border border-dashed p-5">
      <span className="text-muted-foreground">{icon}</span>
      <p className="text-muted-foreground text-sm">{text}</p>
      {action}
    </div>
  );
}
