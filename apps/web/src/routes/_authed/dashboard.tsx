import { useQuery } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { authClient } from "@/lib/auth-client";
import { listProjects } from "@/lib/projects";

export const Route = createFileRoute("/_authed/dashboard")({
  component: DashboardRoute,
});

function DashboardRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;
  const projects = useQuery({
    queryKey: ["projects", organizationId],
    queryFn: listProjects,
    enabled: Boolean(organizationId),
  });
  const projectList = projects.data ?? [];

  return (
    <section className="flex flex-1 flex-col p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Overview</h1>
        <p className="mt-2 text-muted-foreground text-sm">
          Projects in {activeOrganization?.name ?? "this workspace"}.
        </p>
      </div>

      <div className="mt-6">
        {projects.isPending ? (
          <p className="text-muted-foreground text-sm">Loading projects…</p>
        ) : projectList.length === 0 ? (
          <p className="text-muted-foreground text-sm">
            No projects yet. Create one to get started.
          </p>
        ) : (
          <ul className="flex flex-col gap-2">
            {projectList.map((project) => (
              <li
                key={project.id}
                className="rounded-lg border bg-card px-4 py-3 text-card-foreground"
              >
                <p className="font-medium">{project.name}</p>
                <p className="text-muted-foreground text-xs">{project.slug}</p>
              </li>
            ))}
          </ul>
        )}
      </div>
    </section>
  );
}
