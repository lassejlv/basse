import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { FormEvent, useState } from "react";
import { Button } from "@/components/ui/button";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
import { createProject, listProjects } from "@/lib/projects";

export const Route = createFileRoute("/_authed/projects/")({
  component: ProjectsRoute,
});

function ProjectsRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;
  const queryClient = useQueryClient();
  const queryKey = ["projects", organizationId];
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const projects = useQuery({
    queryKey,
    queryFn: listProjects,
    enabled: Boolean(organizationId),
  });

  const add = useMutation({
    mutationFn: () => createProject(name),
    onSuccess: async () => {
      setName("");
      setError(null);
      await queryClient.invalidateQueries({ queryKey });
    },
    onError: (e: Error) => setError(e.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  const projectList = projects.data ?? [];

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <div className="max-w-2xl">
        <h1 className="text-2xl font-semibold tracking-normal md:text-3xl">Projects</h1>
        <p className="mt-2 text-muted-foreground text-sm">
          Each project has environments; apps deploy into an environment.
        </p>
      </div>

      <div className="max-w-2xl">
        {projects.isPending ? (
          <p className="text-muted-foreground text-sm">Loading…</p>
        ) : projectList.length === 0 ? (
          <p className="text-muted-foreground text-sm">No projects yet.</p>
        ) : (
          <ul className="flex flex-col gap-2">
            {projectList.map((project) => (
              <li key={project.id}>
                <Link
                  to="/projects/$projectId"
                  params={{ projectId: project.id }}
                  className="flex items-center justify-between rounded-lg border bg-card px-4 py-3 hover:bg-accent/40"
                >
                  <span className="font-medium">{project.name}</span>
                  <span className="font-mono text-muted-foreground text-xs">{project.slug}</span>
                </Link>
              </li>
            ))}
          </ul>
        )}
      </div>

      <form className="max-w-2xl space-y-4 rounded-lg border bg-card p-6" onSubmit={handleSubmit}>
        <h2 className="text-lg font-semibold">New project</h2>
        <div className="space-y-2">
          <Label htmlFor="project-name">Name</Label>
          <Input
            id="project-name"
            value={name}
            onChange={(event) => setName(event.currentTarget.value)}
            placeholder="my-saas"
            required
          />
        </div>
        {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        <Button disabled={!organizationId} loading={add.isPending} type="submit">
          Create project
        </Button>
      </form>
    </section>
  );
}
