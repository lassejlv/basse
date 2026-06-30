import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute } from "@tanstack/react-router";
import { ChevronRightIcon, FolderIcon, PlusIcon } from "lucide-react";
import { FormEvent, useState } from "react";
import type { ProjectListItem } from "@basse/shared";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  Dialog,
  DialogClose,
  DialogDescription,
  DialogFooter,
  DialogHeader,
  DialogPanel,
  DialogPopup,
  DialogTitle,
  DialogTrigger,
} from "@/components/ui/dialog";
import {
  Empty,
  EmptyContent,
  EmptyDescription,
  EmptyHeader,
  EmptyMedia,
  EmptyTitle,
} from "@/components/ui/empty";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { authClient } from "@/lib/auth-client";
import { relativeTime } from "@/lib/format";
import { createProject, listProjects } from "@/lib/projects";
import { toast } from "@/lib/toast";

export const Route = createFileRoute("/_authed/projects/")({
  component: ProjectsRoute,
});

function ProjectsRoute() {
  const { data: activeOrganization } = authClient.useActiveOrganization();
  const organizationId = activeOrganization?.id;

  const projects = useQuery({
    queryKey: ["projects", organizationId],
    queryFn: listProjects,
    enabled: Boolean(organizationId),
  });

  const projectList = projects.data ?? [];

  return (
    <section className="flex flex-1 flex-col gap-8 p-4 md:p-6">
      <header className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="font-semibold text-2xl tracking-tight md:text-3xl">Projects</h1>
          <p className="mt-1.5 max-w-prose text-muted-foreground text-sm">
            Group your apps by project and environment. Every project starts with a production
            environment.
          </p>
        </div>
        <CreateProjectDialog organizationId={organizationId} />
      </header>

      {projects.isPending ? (
        <ProjectGrid>
          {Array.from({ length: 3 }).map((_, index) => (
            <div
              key={index}
              className="h-32 animate-pulse rounded-2xl border bg-muted/40"
              aria-hidden
            />
          ))}
        </ProjectGrid>
      ) : projectList.length === 0 ? (
        <Empty className="rounded-2xl border border-dashed">
          <EmptyHeader>
            <EmptyMedia variant="icon">
              <FolderIcon />
            </EmptyMedia>
            <EmptyTitle>No projects yet</EmptyTitle>
            <EmptyDescription>Create your first project to start deploying apps.</EmptyDescription>
          </EmptyHeader>
          <EmptyContent>
            <CreateProjectDialog organizationId={organizationId} />
          </EmptyContent>
        </Empty>
      ) : (
        <ProjectGrid>
          {projectList.map((project) => (
            <ProjectCard key={project.id} project={project} />
          ))}
        </ProjectGrid>
      )}
    </section>
  );
}

function ProjectGrid({ children }: { children: React.ReactNode }) {
  return <div className="grid gap-4 sm:grid-cols-2 xl:grid-cols-3">{children}</div>;
}

function ProjectCard({ project }: { project: ProjectListItem }) {
  return (
    <Card
      className="group min-h-32 p-5 transition-[border-color,box-shadow] hover:border-foreground/15 hover:shadow-sm"
      render={<Link params={{ projectId: project.id }} to="/projects/$projectId" />}
    >
      <div className="flex items-start gap-3">
        <span className="flex size-9 shrink-0 items-center justify-center rounded-lg border bg-background text-muted-foreground">
          <FolderIcon className="size-4.5" />
        </span>
        <div className="min-w-0 flex-1">
          <p className="truncate font-medium">{project.name}</p>
          <p className="truncate font-mono text-muted-foreground text-xs">{project.slug}</p>
        </div>
        <ChevronRightIcon className="size-4 shrink-0 text-muted-foreground/50 transition group-hover:translate-x-0.5 group-hover:text-muted-foreground" />
      </div>
      <dl className="mt-auto flex items-center gap-2 pt-5 font-mono text-muted-foreground text-xs">
        <dd>
          {project.appCount} {project.appCount === 1 ? "app" : "apps"}
        </dd>
        <span aria-hidden>·</span>
        <dd>
          {project.environmentCount} {project.environmentCount === 1 ? "env" : "envs"}
        </dd>
        <span aria-hidden>·</span>
        <dd>{relativeTime(project.createdAt)}</dd>
      </dl>
    </Card>
  );
}

function CreateProjectDialog({ organizationId }: { organizationId: string | undefined }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () => createProject(name),
    onSuccess: async () => {
      setName("");
      setError(null);
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["projects", organizationId] });
      toast.success("Project created");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    add.mutate();
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) setError(null);
      }}
    >
      <DialogTrigger
        disabled={!organizationId}
        render={
          <Button>
            <PlusIcon />
            New project
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-md">
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New project</DialogTitle>
            <DialogDescription>
              Pick a name. We'll create a production environment to deploy into.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-2">
            <Label htmlFor="project-name">Name</Label>
            <Input
              autoFocus
              id="project-name"
              onChange={(event) => setName(event.currentTarget.value)}
              placeholder="my-saas"
              required
              value={name}
            />
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button disabled={!name.trim()} loading={add.isPending} type="submit">
              Create project
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
