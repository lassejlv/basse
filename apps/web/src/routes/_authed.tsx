import { Link, Outlet, createFileRoute, redirect, useLocation } from "@tanstack/react-router";
import { CloudIcon, LayoutDashboardIcon, ServerIcon, SettingsIcon } from "lucide-react";
import { useEffect } from "react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Select, SelectButton, SelectItem, SelectPopup, SelectValue } from "@/components/ui/select";
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
    : pathname === "/settings"
      ? "Settings"
      : "Overview";
  const user = session.user;
  const displayName = user.name || user.email;
  const initials = getInitials(displayName);
  const { data: organizations } = authClient.useListOrganizations();
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const workspaceList = organizations ?? [];
  const selectedWorkspaceId = activeOrganization?.id ?? workspaceList[0]?.id ?? "";

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
                <span className="flex size-8 shrink-0 items-center justify-center rounded-lg border border-sidebar-border bg-sidebar-primary text-sidebar-primary-foreground">
                  <CloudIcon className="size-4" />
                </span>
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
              <SelectButton
                className="h-9 w-full min-w-0 bg-background"
                disabled={!workspaceList.length}
              >
                <SelectValue placeholder="Select workspace" />
              </SelectButton>
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
              </SidebarMenu>
            </SidebarGroupContent>
          </SidebarGroup>

          <SidebarGroup>
            <SidebarGroupLabel>Infrastructure</SidebarGroupLabel>
            <SidebarGroupContent>
              <SidebarMenu>
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
