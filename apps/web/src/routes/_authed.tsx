import { useQuery } from "@tanstack/react-query";
import { Link, Outlet, createFileRoute, redirect, useLocation } from "@tanstack/react-router";
import {
  BellIcon,
  FolderIcon,
  KeyRoundIcon,
  LayoutDashboardIcon,
  ServerIcon,
  SettingsIcon,
} from "lucide-react";
import { useEffect } from "react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sidebar,
  SidebarContent,
  SidebarFooter,
  SidebarGroup,
  SidebarGroupContent,
  SidebarGroupLabel,
  SidebarHeader,
  SidebarInset,
  SidebarMenu,
  SidebarMenuButton,
  SidebarMenuItem,
  SidebarProvider,
  SidebarRail,
  SidebarTrigger,
} from "@/components/ui/sidebar";
import { getAlertsOverview } from "@/lib/alerts";
import { authClient } from "@/lib/auth-client";

export const Route = createFileRoute("/_authed")({
  beforeLoad: async ({ location }) => {
    const session = await authClient.getSession();

    if (!session.data?.user) {
      throw redirect({
        to: "/login",
        search: {
          redirect: location.href,
        },
      });
    }

    return {
      session: session.data,
    };
  },
  component: AuthedLayout,
});

function AuthedLayout() {
  const { session } = Route.useRouteContext();
  const pathname = useLocation({ select: (location) => location.pathname });
  const pageTitle = pathname.startsWith("/servers")
    ? "Servers"
    : pathname.startsWith("/projects") || pathname.startsWith("/apps")
      ? "Projects"
      : pathname === "/alerts"
        ? "Alerts"
        : pathname === "/secrets"
          ? "Secrets"
          : pathname === "/settings"
            ? "Settings"
            : "Overview";
  const user = session.user;
  const displayName = user.name || user.email;
  const initials = getInitials(displayName);
  const { data: organizations } = authClient.useListOrganizations();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const alertsOverview = useQuery({
    queryKey: ["alerts-overview", activeOrganization?.id],
    queryFn: getAlertsOverview,
    enabled: Boolean(activeOrganization?.id),
    refetchInterval: 15_000,
  });
  const workspaceList = organizations ?? [];
  const selectedWorkspaceId = activeOrganization?.id ?? workspaceList[0]?.id ?? "";
  const activeAlertCount =
    (alertsOverview.data?.openCount ?? 0) + (alertsOverview.data?.acknowledgedCount ?? 0);

  const setActiveWorkspace = (organizationId: string) => {
    void authClient.organization.setActive({ organizationId });
  };

  useEffect(() => {
    if (activeOrganization || !workspaceList[0]) {
      return;
    }

    setActiveWorkspace(workspaceList[0].id);
  }, [activeOrganization, workspaceList]);

  return (
    <SidebarProvider>
      <Sidebar collapsible="icon" variant="floating">
        <SidebarHeader className="p-3">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton
                className="h-11 gap-3"
                render={<Link to="/dashboard" />}
                size="lg"
                tooltip="Basse"
              >
                <span className="flex min-w-0 flex-col">
                  <span className="truncate font-semibold">Basse</span>
                  <span className="truncate text-muted-foreground text-xs">Personal cloud</span>
                </span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
          <div className="group-data-[collapsible=icon]:hidden">
            <Select
              value={selectedWorkspaceId}
              onValueChange={(organizationId) => {
                if (!organizationId) {
                  return;
                }

                setActiveWorkspace(organizationId);
              }}
            >
              <SelectTrigger
                className="h-9 w-full min-w-0 bg-background"
                disabled={!workspaceList.length}
              >
                <SelectValue placeholder="Select workspace">
                  {(value: string) =>
                    workspaceList.find((o) => o.id === value)?.name ?? "Select workspace"
                  }
                </SelectValue>
              </SelectTrigger>
              <SelectPopup>
                {workspaceList.map((organization) => (
                  <SelectItem key={organization.id} value={organization.id}>
                    {organization.name}
                  </SelectItem>
                ))}
              </SelectPopup>
            </Select>
          </div>
        </SidebarHeader>

        <SidebarContent>
          <SidebarGroup>
            <SidebarGroupLabel>General</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname === "/dashboard"}
                    render={<Link to="/dashboard" />}
                    tooltip="Overview"
                  >
                    <LayoutDashboardIcon />
                    <span>Overview</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname === "/alerts"}
                    render={<Link to="/alerts" />}
                    tooltip="Alerts"
                  >
                    <BellIcon />
                    <span>Alerts</span>
                    {activeAlertCount > 0 ? (
                      <span className="ml-auto rounded-sm bg-destructive px-1.5 py-0.5 text-[0.625rem] text-white group-data-[collapsible=icon]:hidden">
                        {activeAlertCount}
                      </span>
                    ) : null}
                  </SidebarMenuButton>
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>

          <SidebarGroup>
            <SidebarGroupLabel>Infrastructure</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname.startsWith("/projects") || pathname.startsWith("/apps")}
                    render={<Link to="/projects" />}
                    tooltip="Projects"
                  >
                    <FolderIcon />
                    <span>Projects</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname.startsWith("/servers")}
                    render={<Link to="/servers" />}
                    tooltip="Servers"
                  >
                    <ServerIcon />
                    <span>Servers</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>

          <SidebarGroup>
            <SidebarGroupLabel>Workspace</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname === "/secrets"}
                    render={
                      <Link
                        to="/secrets"
                        search={{
                          code: undefined,
                          installation_id: undefined,
                          setup_action: undefined,
                          state: undefined,
                        }}
                      />
                    }
                    tooltip="Secrets"
                  >
                    <KeyRoundIcon />
                    <span>Secrets</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
                <SidebarMenuItem>
                  <SidebarMenuButton
                    isActive={pathname === "/settings"}
                    render={<Link to="/settings" />}
                    tooltip="Settings"
                  >
                    <SettingsIcon />
                    <span>Settings</span>
                  </SidebarMenuButton>
                </SidebarMenuItem>
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>
        </SidebarContent>

        <SidebarFooter className="p-3">
          <SidebarMenu>
            <SidebarMenuItem>
              <SidebarMenuButton className="h-11 gap-3" tooltip={displayName}>
                <Avatar className="size-8 rounded-lg">
                  {user.image ? <AvatarImage alt={displayName} src={user.image} /> : null}
                  <AvatarFallback className="rounded-lg bg-sidebar-primary text-sidebar-primary-foreground">
                    {initials}
                  </AvatarFallback>
                </Avatar>
                <span className="flex min-w-0 flex-col">
                  <span className="truncate font-medium">{displayName}</span>
                  <span className="truncate text-muted-foreground text-xs">{user.email}</span>
                </span>
              </SidebarMenuButton>
            </SidebarMenuItem>
          </SidebarMenu>
        </SidebarFooter>
        <SidebarRail />
      </Sidebar>

      <SidebarInset>
        <header className="sticky top-0 z-10 flex h-14 shrink-0 items-center gap-3 border-b bg-background/94 px-4 backdrop-blur md:px-6">
          <SidebarTrigger />
          <p className="truncate font-medium text-sm">{pageTitle}</p>
          <ThemeToggle className="ml-auto" />
        </header>

        <Outlet />
      </SidebarInset>
    </SidebarProvider>
  );
}

function getInitials(value: string) {
  return value
    .split(/\s+/)
    .filter(Boolean)
    .slice(0, 2)
    .map((part) => part[0]?.toUpperCase())
    .join("");
}
