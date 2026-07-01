import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link, createFileRoute, useNavigate } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  BoxIcon,
  ChevronDownIcon,
  DownloadIcon,
  HardDriveIcon,
  HistoryIcon,
  KeyRoundIcon,
  MaximizeIcon,
  MinusIcon,
  PencilIcon,
  PlusIcon,
  RefreshCwIcon,
  SettingsIcon,
  TrashIcon,
} from "lucide-react";
import {
  FormEvent,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import type {
  App,
  AppBuildMode,
  AppBuildRunner,
  AppKind,
  AppSourceType,
  DatabaseKind,
  ImportableDockerContainer,
} from "@basse/shared";
import { AppPanel } from "@/components/app/app-panel";
import { DatabaseIcon, databaseEngineLabel } from "@/components/database-icon";
import { StatusDot, deployState } from "@/components/deploy-status";
import { GitHubRepositorySelect } from "@/components/github-repository-select";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Checkbox } from "@/components/ui/checkbox";
import {
  ProjectStagedChangesBar,
  ProjectStagedChangesHistory,
} from "@/components/staged-changes-bar";
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
import {
  Menu,
  MenuItem,
  MenuPopup,
  MenuRadioGroup,
  MenuRadioItem,
  MenuSeparator,
  MenuTrigger,
} from "@/components/ui/menu";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import {
  Sheet,
  SheetDescription,
  SheetHeader,
  SheetPanel,
  SheetPopup,
  SheetTitle,
} from "@/components/ui/sheet";
import { Textarea } from "@/components/ui/textarea";
import {
  createApp,
  importDockerContainer,
  listApps,
  listImportableDockerContainers,
} from "@/lib/apps";
import { getProjectChangeHistory, getProjectChanges } from "@/lib/changes";
import { triggerDeploy } from "@/lib/deployments";
import { createEnvironment, listEnvironments } from "@/lib/environments";
import {
  listEnvironmentSharedEnvVars,
  listProjectSharedEnvVars,
  revealEnvironmentSharedEnvVars,
  revealProjectSharedEnvVars,
  setEnvironmentSharedEnvVars,
  setProjectSharedEnvVars,
  type SharedEnvVarMasked,
  type SharedEnvVarPlain,
} from "@/lib/env-vars";
import { listGitHubRepositories } from "@/lib/github";
import { parseDotenv, serializeDotenv } from "@/lib/dotenv";
import { deleteProject, getProject } from "@/lib/projects";
import { listServers } from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/_authed/projects/$projectId")({
  component: ProjectDetailRoute,
  validateSearch: (search: Record<string, unknown>): { app?: string; tab?: string } => ({
    app: typeof search.app === "string" ? search.app : undefined,
    tab: typeof search.tab === "string" ? search.tab : undefined,
  }),
});

type SettingsDialog = "project-vars" | "environment-vars";

function envStorageKey(projectId: string): string {
  return `basse:project-env:${projectId}`;
}

function ProjectDetailRoute() {
  const { projectId } = Route.useParams();
  const search = Route.useSearch();
  const navigate = useNavigate({ from: Route.fullPath });
  const queryClient = useQueryClient();
  const [activeEnv, setActiveEnv] = useState<string | null>(() =>
    localStorage.getItem(envStorageKey(projectId)),
  );
  const [settingsDialog, setSettingsDialog] = useState<SettingsDialog | null>(null);
  const [newEnvOpen, setNewEnvOpen] = useState(false);
  const [activityOpen, setActivityOpen] = useState(false);

  const selectedAppId = search.app ?? null;

  function selectApp(appId: string | null) {
    void navigate({
      search: (prev) => ({
        app: appId ?? undefined,
        tab: appId && appId === prev.app ? prev.tab : undefined,
      }),
      replace: appId === null,
    });
  }

  function setPanelTab(tab: string) {
    void navigate({
      search: (prev) => ({ ...prev, tab: tab === "deployments" ? undefined : tab }),
      replace: true,
    });
  }

  // Escape closes the app panel — unless a dialog/sheet is open (it owns Escape).
  useEffect(() => {
    if (!selectedAppId) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      if (document.querySelector('[data-slot="dialog-popup"], [data-slot="sheet-popup"]')) return;
      selectApp(null);
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [selectedAppId]);

  const project = useQuery({
    queryKey: ["project", projectId],
    queryFn: () => getProject(projectId),
  });
  const environments = useQuery({
    queryKey: ["environments", projectId],
    queryFn: () => listEnvironments(projectId),
  });
  const projectChanges = useQuery({
    queryKey: ["project-changes", projectId],
    queryFn: () => getProjectChanges(projectId),
  });
  const projectChangeHistory = useQuery({
    queryKey: ["project-change-history", projectId],
    queryFn: () => getProjectChangeHistory(projectId),
  });

  const envList = environments.data ?? [];
  const selectedEnv =
    (activeEnv && envList.some((env) => env.id === activeEnv) ? activeEnv : null) ??
    envList.find((env) => env.isDefault)?.id ??
    envList[0]?.id ??
    null;
  const selectedEnvName = envList.find((env) => env.id === selectedEnv)?.name ?? "Environment";

  function switchEnvironment(envId: string) {
    setActiveEnv(envId);
    try {
      localStorage.setItem(envStorageKey(projectId), envId);
    } catch {
      // Persistence is a convenience only.
    }
    if (selectedAppId) selectApp(null);
  }

  const stagedChangeCount = (projectChanges.data?.changes ?? []).length;

  const removeProject = useMutation({
    mutationFn: () => deleteProject(projectId),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: ["projects"] });
      await navigate({ to: "/projects" });
      toast.success("Project deleted");
    },
    onError: (mutationError: Error) =>
      toast.error("Couldn't delete project", { description: mutationError.message }),
  });

  function confirmDeleteProject() {
    const name = project.data?.name ?? "this project";
    if (
      !window.confirm(
        `Delete ${name}? This removes its environments, apps, and running app containers.`,
      )
    ) {
      return;
    }
    removeProject.mutate();
  }

  if (project.isPending) {
    return <p className="p-4 text-muted-foreground text-sm md:p-6">Loading…</p>;
  }
  if (project.isError || !project.data) {
    return <p className="p-4 text-destructive-foreground text-sm md:p-6">Project not found.</p>;
  }

  return (
    <section className="relative flex min-h-0 flex-1 flex-col overflow-hidden">
      <div className="flex flex-wrap items-center gap-2 border-b px-3 py-2 md:px-4">
        <Button
          aria-label="Back to projects"
          render={<Link to="/projects" />}
          size="icon-sm"
          variant="ghost"
        >
          <ArrowLeftIcon />
        </Button>
        <h1 className="min-w-0 truncate font-semibold text-sm tracking-tight">
          {project.data.name}
        </h1>
        <span aria-hidden className="text-muted-foreground/40">
          /
        </span>
        {environments.isPending ? (
          <div className="h-7 w-28 animate-pulse rounded-lg bg-muted/50" aria-hidden />
        ) : (
          <Menu>
            <MenuTrigger
              render={
                <Button className="gap-1.5 font-normal" size="sm" variant="ghost">
                  {selectedEnvName}
                  <ChevronDownIcon className="size-3.5 text-muted-foreground" />
                </Button>
              }
            />
            <MenuPopup align="start">
              <MenuRadioGroup
                onValueChange={(value) => switchEnvironment(value as string)}
                value={selectedEnv ?? ""}
              >
                {envList.map((env) => (
                  <MenuRadioItem key={env.id} value={env.id}>
                    {env.name}
                  </MenuRadioItem>
                ))}
              </MenuRadioGroup>
              <MenuSeparator />
              <MenuItem onClick={() => setNewEnvOpen(true)}>
                <PlusIcon />
                New environment
              </MenuItem>
            </MenuPopup>
          </Menu>
        )}
        <div className="ml-auto flex items-center gap-2">
          <Button onClick={() => setActivityOpen(true)} size="sm" variant="outline">
            <HistoryIcon />
            Activity
            {stagedChangeCount > 0 ? (
              <span className="flex size-4.5 items-center justify-center rounded-full bg-primary font-semibold text-[0.65rem] text-primary-foreground">
                {stagedChangeCount}
              </span>
            ) : null}
          </Button>
          <Menu>
            <MenuTrigger
              render={
                <Button aria-label="Project settings" size="icon" variant="outline">
                  <SettingsIcon />
                </Button>
              }
            />
            <MenuPopup align="end">
              <MenuItem onClick={() => setSettingsDialog("project-vars")}>
                <KeyRoundIcon />
                Shared variables
              </MenuItem>
              <MenuItem
                disabled={!selectedEnv}
                onClick={() => setSettingsDialog("environment-vars")}
              >
                <KeyRoundIcon />
                {selectedEnvName} variables
              </MenuItem>
              <MenuSeparator />
              <MenuItem onClick={confirmDeleteProject} variant="destructive">
                <TrashIcon />
                Delete project
              </MenuItem>
            </MenuPopup>
          </Menu>
          {selectedEnv ? (
            <>
              <ImportContainerDialog environmentId={selectedEnv} />
              <CreateAppDialog environmentId={selectedEnv} />
            </>
          ) : null}
        </div>
      </div>

      <div className="relative min-h-0 flex-1">
        {selectedEnv ? (
          <EnvironmentCanvas
            environmentId={selectedEnv}
            onSelectApp={selectApp}
            selectedAppId={selectedAppId}
          />
        ) : environments.isPending ? null : (
          <p className="p-6 text-muted-foreground text-sm">No environments in this project.</p>
        )}
        {selectedAppId ? (
          <AppPanel
            appId={selectedAppId}
            key={selectedAppId}
            onClose={() => selectApp(null)}
            onTabChange={setPanelTab}
            tab={search.tab}
          />
        ) : null}
      </div>

      <SharedEnvVarsDialog
        description="Available to every app in this project as {{shared.KEY}}."
        listQueryKey={["project-shared-env-vars", projectId]}
        listVars={() => listProjectSharedEnvVars(projectId)}
        onOpenChange={(open) => setSettingsDialog(open ? "project-vars" : null)}
        open={settingsDialog === "project-vars"}
        revealVars={() => revealProjectSharedEnvVars(projectId)}
        saveVars={(vars) => setProjectSharedEnvVars(projectId, vars)}
        title="Shared variables"
      />
      {selectedEnv ? (
        <SharedEnvVarsDialog
          description="Available to apps in the selected environment as {{env.KEY}}."
          listQueryKey={["environment-shared-env-vars", selectedEnv]}
          listVars={() => listEnvironmentSharedEnvVars(selectedEnv)}
          onOpenChange={(open) => setSettingsDialog(open ? "environment-vars" : null)}
          open={settingsDialog === "environment-vars"}
          revealVars={() => revealEnvironmentSharedEnvVars(selectedEnv)}
          saveVars={(vars) => setEnvironmentSharedEnvVars(selectedEnv, vars)}
          title={`${selectedEnvName} variables`}
        />
      ) : null}
      <Sheet onOpenChange={setActivityOpen} open={activityOpen}>
        <SheetPopup className="max-w-md" side="right">
          <SheetHeader>
            <SheetTitle>Activity</SheetTitle>
            <SheetDescription>
              Staged changes and applied history for this project.
            </SheetDescription>
          </SheetHeader>
          <SheetPanel className="space-y-5">
            <ProjectStagedChangesBar
              changes={projectChanges.data?.changes ?? []}
              projectId={projectId}
            />
            <ProjectStagedChangesHistory
              entries={projectChangeHistory.data ?? []}
              isPending={projectChangeHistory.isPending}
            />
          </SheetPanel>
        </SheetPopup>
      </Sheet>
      <NewEnvironmentDialog
        onOpenChange={setNewEnvOpen}
        open={newEnvOpen}
        projectId={projectId}
      />
    </section>
  );
}

// --- Canvas ---------------------------------------------------------------

type CanvasView = { x: number; y: number; scale: number };
type NodePosition = { x: number; y: number };

const NODE_WIDTH = 248;
const NODE_BASE_HEIGHT = 92;
const VOLUME_TAB_HEIGHT = 30;
const GRID_SIZE = 24;
const MIN_SCALE = 0.35;
const MAX_SCALE = 1.75;
const FIT_PADDING = 80;

function clampScale(scale: number): number {
  return Math.min(MAX_SCALE, Math.max(MIN_SCALE, scale));
}

function autoPosition(index: number): NodePosition {
  const column = index % 3;
  const row = Math.floor(index / 3);
  return { x: column * (NODE_WIDTH + 64), y: row * (NODE_BASE_HEIGHT + 84) };
}

function nodeHeight(app: App): number {
  return NODE_BASE_HEIGHT + (app.volumes.length > 0 ? VOLUME_TAB_HEIGHT : 0);
}

function appSourceLabel(app: App): string {
  if (app.appKind === "database" && app.database) {
    return `${databaseEngineLabel(app.database.kind)} ${app.database.version}`.trim();
  }
  if (app.sourceType === "image") return app.imageRef ?? "";
  return app.repositoryUrl.replace(/^https?:\/\//, "");
}

function positionsStorageKey(environmentId: string): string {
  return `basse:canvas:${environmentId}`;
}

function readStoredPositions(environmentId: string): Record<string, NodePosition> {
  try {
    const raw = localStorage.getItem(positionsStorageKey(environmentId));
    if (!raw) return {};
    const parsed = JSON.parse(raw) as Record<string, NodePosition>;
    const valid: Record<string, NodePosition> = {};
    for (const [id, position] of Object.entries(parsed)) {
      if (
        position &&
        typeof position.x === "number" &&
        typeof position.y === "number" &&
        Number.isFinite(position.x) &&
        Number.isFinite(position.y)
      ) {
        valid[id] = position;
      }
    }
    return valid;
  } catch {
    return {};
  }
}

function writeStoredPositions(environmentId: string, positions: Record<string, NodePosition>) {
  try {
    localStorage.setItem(positionsStorageKey(environmentId), JSON.stringify(positions));
  } catch {
    // Position persistence is a convenience; ignore quota/privacy-mode failures.
  }
}

type PointerGesture = {
  pointerId: number;
  startX: number;
  startY: number;
  originX: number;
  originY: number;
  moved: boolean;
};

function EnvironmentCanvas({
  environmentId,
  selectedAppId,
  onSelectApp,
}: {
  environmentId: string;
  selectedAppId: string | null;
  onSelectApp: (appId: string | null) => void;
}) {
  const viewportRef = useRef<HTMLDivElement>(null);
  const panRef = useRef<PointerGesture | null>(null);
  const fittedRef = useRef(false);
  const [view, setView] = useState<CanvasView>({ x: 0, y: 0, scale: 1 });
  const [panning, setPanning] = useState(false);
  const [stored, setStored] = useState<Record<string, NodePosition>>(() =>
    readStoredPositions(environmentId),
  );

  const apps = useQuery({
    queryKey: ["apps", environmentId],
    queryFn: () => listApps(environmentId),
    refetchInterval: 15_000,
  });
  const appList = useMemo(() => apps.data ?? [], [apps.data]);

  const nodes = useMemo(
    () =>
      appList.map((app, index) => ({
        app,
        position: stored[app.id] ?? autoPosition(index),
      })),
    [appList, stored],
  );

  useEffect(() => {
    setStored(readStoredPositions(environmentId));
    fittedRef.current = false;
  }, [environmentId]);

  const fitView = useCallback(() => {
    const viewport = viewportRef.current;
    if (!viewport || nodes.length === 0) return;
    const rect = viewport.getBoundingClientRect();
    let minX = Infinity;
    let minY = Infinity;
    let maxX = -Infinity;
    let maxY = -Infinity;
    for (const node of nodes) {
      minX = Math.min(minX, node.position.x);
      minY = Math.min(minY, node.position.y);
      maxX = Math.max(maxX, node.position.x + NODE_WIDTH);
      maxY = Math.max(maxY, node.position.y + nodeHeight(node.app));
    }
    const width = Math.max(maxX - minX, 1);
    const height = Math.max(maxY - minY, 1);
    const scale = clampScale(
      Math.min(
        (rect.width - FIT_PADDING * 2) / width,
        (rect.height - FIT_PADDING * 2) / height,
        1,
      ),
    );
    setView({
      scale,
      x: (rect.width - width * scale) / 2 - minX * scale,
      y: (rect.height - height * scale) / 2 - minY * scale,
    });
  }, [nodes]);

  useLayoutEffect(() => {
    if (fittedRef.current || apps.isPending) return;
    fittedRef.current = true;
    fitView();
  }, [apps.isPending, fitView]);

  // Wheel: trackpad scroll pans, pinch (ctrl/cmd + wheel) zooms toward cursor.
  useEffect(() => {
    const viewport = viewportRef.current;
    if (!viewport) return;
    function onWheel(event: WheelEvent) {
      event.preventDefault();
      if (event.ctrlKey || event.metaKey) {
        const rect = viewport!.getBoundingClientRect();
        const pointX = event.clientX - rect.left;
        const pointY = event.clientY - rect.top;
        setView((current) => {
          const scale = clampScale(current.scale * Math.exp(-event.deltaY * 0.01));
          const ratio = scale / current.scale;
          return {
            scale,
            x: pointX - (pointX - current.x) * ratio,
            y: pointY - (pointY - current.y) * ratio,
          };
        });
      } else {
        setView((current) => ({
          ...current,
          x: current.x - event.deltaX,
          y: current.y - event.deltaY,
        }));
      }
    }
    viewport.addEventListener("wheel", onWheel, { passive: false });
    return () => viewport.removeEventListener("wheel", onWheel);
  }, []);

  function zoomBy(factor: number) {
    const viewport = viewportRef.current;
    if (!viewport) return;
    const rect = viewport.getBoundingClientRect();
    const centerX = rect.width / 2;
    const centerY = rect.height / 2;
    setView((current) => {
      const scale = clampScale(current.scale * factor);
      const ratio = scale / current.scale;
      return {
        scale,
        x: centerX - (centerX - current.x) * ratio,
        y: centerY - (centerY - current.y) * ratio,
      };
    });
  }

  function moveNode(appId: string, position: NodePosition) {
    setStored((current) => ({ ...current, [appId]: position }));
  }

  function persistPositions() {
    setStored((current) => {
      writeStoredPositions(environmentId, current);
      return current;
    });
  }

  return (
    <div className="absolute inset-0 overflow-hidden bg-background">
      <div
        className={cn(
          "absolute inset-0 touch-none select-none",
          panning ? "cursor-grabbing" : "cursor-grab",
        )}
        onPointerDown={(event) => {
          if (event.button !== 0 && event.button !== 1) return;
          panRef.current = {
            pointerId: event.pointerId,
            startX: event.clientX,
            startY: event.clientY,
            originX: view.x,
            originY: view.y,
            moved: false,
          };
          setPanning(true);
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          const gesture = panRef.current;
          if (!gesture || gesture.pointerId !== event.pointerId) return;
          const deltaX = event.clientX - gesture.startX;
          const deltaY = event.clientY - gesture.startY;
          if (!gesture.moved && Math.hypot(deltaX, deltaY) < 3) return;
          gesture.moved = true;
          setView((current) => ({
            ...current,
            x: gesture.originX + deltaX,
            y: gesture.originY + deltaY,
          }));
        }}
        onPointerUp={(event) => {
          const gesture = panRef.current;
          if (gesture?.pointerId === event.pointerId) {
            if (!gesture.moved) onSelectApp(null);
            panRef.current = null;
            setPanning(false);
          }
        }}
        onPointerCancel={() => {
          panRef.current = null;
          setPanning(false);
        }}
        ref={viewportRef}
        style={{
          backgroundImage: "radial-gradient(var(--color-border) 1px, transparent 1px)",
          backgroundSize: `${GRID_SIZE * view.scale}px ${GRID_SIZE * view.scale}px`,
          backgroundPosition: `${view.x}px ${view.y}px`,
        }}
      >
        <div
          className="absolute top-0 left-0 will-change-transform"
          style={{
            transform: `translate(${view.x}px, ${view.y}px) scale(${view.scale})`,
            transformOrigin: "0 0",
          }}
        >
          {nodes.map(({ app, position }) => (
            <CanvasNode
              app={app}
              key={app.id}
              onMove={moveNode}
              onMoveEnd={persistPositions}
              onSelect={onSelectApp}
              position={position}
              scale={view.scale}
              selected={app.id === selectedAppId}
            />
          ))}
        </div>
      </div>

      <div
        aria-hidden
        className="pointer-events-none absolute inset-0 bg-[radial-gradient(130%_130%_at_50%_40%,transparent_60%,var(--color-background)_100%)] opacity-60"
      />

      {apps.isPending ? (
        <div className="absolute inset-0 grid place-items-center">
          <p className="text-muted-foreground text-sm">Loading apps…</p>
        </div>
      ) : apps.isError ? (
        <div className="absolute inset-0 grid place-items-center">
          <p className="text-destructive-foreground text-sm">{toMessage(apps.error)}</p>
        </div>
      ) : appList.length === 0 ? (
        <div className="absolute inset-0 z-10 grid place-items-center p-6">
          <Empty className="max-w-md rounded-2xl border border-dashed bg-card/80 backdrop-blur">
            <EmptyHeader>
              <EmptyMedia variant="icon">
                <BoxIcon />
              </EmptyMedia>
              <EmptyTitle>No apps in this environment</EmptyTitle>
              <EmptyDescription>
                Deploy from a Git repository or a prebuilt Docker image.
              </EmptyDescription>
            </EmptyHeader>
            <EmptyContent className="flex flex-wrap items-center justify-center gap-2">
              <ImportContainerDialog environmentId={environmentId} />
              <CreateAppDialog environmentId={environmentId} />
            </EmptyContent>
          </Empty>
        </div>
      ) : null}

      {appList.length > 0 ? (
        <div className="absolute bottom-4 left-4 z-10 flex flex-col divide-y overflow-hidden rounded-lg border bg-card shadow-sm">
          <Button
            aria-label="Zoom in"
            className="rounded-none"
            onClick={() => zoomBy(1.25)}
            size="icon-sm"
            variant="ghost"
          >
            <PlusIcon />
          </Button>
          <Button
            aria-label="Zoom out"
            className="rounded-none"
            onClick={() => zoomBy(1 / 1.25)}
            size="icon-sm"
            variant="ghost"
          >
            <MinusIcon />
          </Button>
          <Button
            aria-label="Fit view"
            className="rounded-none"
            onClick={fitView}
            size="icon-sm"
            variant="ghost"
          >
            <MaximizeIcon />
          </Button>
        </div>
      ) : null}

    </div>
  );
}

function CanvasNode({
  app,
  position,
  scale,
  selected,
  onSelect,
  onMove,
  onMoveEnd,
}: {
  app: App;
  position: NodePosition;
  scale: number;
  selected: boolean;
  onSelect: (appId: string) => void;
  onMove: (appId: string, position: NodePosition) => void;
  onMoveEnd: () => void;
}) {
  const dragRef = useRef<PointerGesture | null>(null);
  const database = app.appKind === "database" ? app.database : null;
  const state = deployState(app.latestDeploymentStatus);
  const volumeLabel =
    app.volumes.length === 1
      ? (app.volumes[0]?.containerPath ?? "")
      : `${app.volumes.length} volumes`;

  return (
    <div className="absolute" style={{ left: position.x, top: position.y, width: NODE_WIDTH }}>
      <button
        className={cn(
          "relative z-10 w-full rounded-xl border bg-card p-3.5 text-left shadow-sm outline-none transition-[border-color,box-shadow]",
          "hover:border-muted-foreground/40 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-1 focus-visible:ring-offset-background",
          selected && "border-primary/50 ring-2 ring-primary/20",
        )}
        onClick={(event) => {
          // Pointer clicks are handled in pointerup (to distinguish drags);
          // detail === 0 means keyboard activation.
          if (event.detail === 0) onSelect(app.id);
        }}
        onPointerCancel={() => {
          dragRef.current = null;
        }}
        onPointerDown={(event) => {
          if (event.button !== 0) return;
          event.stopPropagation();
          dragRef.current = {
            pointerId: event.pointerId,
            startX: event.clientX,
            startY: event.clientY,
            originX: position.x,
            originY: position.y,
            moved: false,
          };
          event.currentTarget.setPointerCapture(event.pointerId);
        }}
        onPointerMove={(event) => {
          const gesture = dragRef.current;
          if (!gesture || gesture.pointerId !== event.pointerId) return;
          const deltaX = event.clientX - gesture.startX;
          const deltaY = event.clientY - gesture.startY;
          if (!gesture.moved && Math.hypot(deltaX, deltaY) < 4) return;
          gesture.moved = true;
          onMove(app.id, {
            x: gesture.originX + deltaX / scale,
            y: gesture.originY + deltaY / scale,
          });
        }}
        onPointerUp={(event) => {
          const gesture = dragRef.current;
          dragRef.current = null;
          if (!gesture || gesture.pointerId !== event.pointerId) return;
          if (gesture.moved) {
            onMoveEnd();
          } else {
            onSelect(app.id);
          }
        }}
        type="button"
      >
        <div className="flex items-center gap-2.5">
          <StatusDot status={app.latestDeploymentStatus} />
          <span className="min-w-0 flex-1 truncate font-medium text-sm">{app.name}</span>
          {database ? (
            <DatabaseIcon className="size-4 shrink-0 opacity-70" kind={database.kind} />
          ) : (
            <BoxIcon className="size-4 shrink-0 text-muted-foreground/70" />
          )}
        </div>
        <p className="mt-1.5 truncate font-mono text-muted-foreground text-xs">
          {appSourceLabel(app)}
        </p>
        <div className="mt-2.5 flex items-center justify-between gap-2">
          <span className="text-muted-foreground text-xs">{state.label}</span>
          <span className="font-mono text-[11px] text-muted-foreground/70">:{app.port}</span>
        </div>
      </button>
      {app.volumes.length > 0 ? (
        <div className="mx-4 flex items-center gap-1.5 rounded-b-lg border border-t-0 bg-card/70 px-2.5 py-1.5 font-mono text-[11px] text-muted-foreground">
          <HardDriveIcon className="size-3 shrink-0" />
          <span className="truncate">{volumeLabel}</span>
        </div>
      ) : null}
    </div>
  );
}

// --- Settings dialogs -------------------------------------------------------

function SharedEnvVarsDialog({
  open,
  onOpenChange,
  title,
  description,
  listQueryKey,
  listVars,
  revealVars,
  saveVars,
}: {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  description: string;
  listQueryKey: unknown[];
  listVars: () => Promise<SharedEnvVarMasked[]>;
  revealVars: () => Promise<SharedEnvVarPlain[]>;
  saveVars: (vars: { key: string; value: string }[]) => Promise<void>;
}) {
  const queryClient = useQueryClient();
  const vars = useQuery({ queryKey: listQueryKey, queryFn: listVars, enabled: open });
  const [editing, setEditing] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [draft, setDraft] = useState("");
  const [error, setError] = useState<string | null>(null);
  const list = vars.data ?? [];

  const save = useMutation({
    mutationFn: () => saveVars(parseDotenv(draft)),
    onSuccess: async () => {
      setError(null);
      setEditing(false);
      toast.success("Shared variables saved");
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: listQueryKey }),
        queryClient.invalidateQueries({ queryKey: ["env-references"] }),
      ]);
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  async function startEdit() {
    setError(null);
    setPreparing(true);
    try {
      setDraft(serializeDotenv(await revealVars()));
      setEditing(true);
    } catch (loadError) {
      setError(loadError instanceof Error ? loadError.message : "Could not load variables.");
    } finally {
      setPreparing(false);
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next);
        if (!next) {
          setEditing(false);
          setError(null);
        }
      }}
    >
      <DialogPopup className="h-fit max-w-lg">
        <DialogHeader>
          <DialogTitle>{title}</DialogTitle>
          <DialogDescription>{description}</DialogDescription>
        </DialogHeader>
        <DialogPanel className="space-y-3">
          {editing ? (
            <>
              <Textarea
                autoFocus
                className="min-h-40 font-mono text-xs leading-relaxed"
                onChange={(event) => setDraft(event.currentTarget.value)}
                spellCheck={false}
                value={draft}
              />
              <p className="text-muted-foreground text-xs">
                One <code className="font-mono">KEY=value</code> per line. App variables can
                reference these values with the token shown above.
              </p>
            </>
          ) : vars.isPending ? (
            <p className="text-muted-foreground text-sm">Loading…</p>
          ) : list.length === 0 ? (
            <p className="rounded-lg border border-dashed px-3 py-5 text-center text-muted-foreground text-sm">
              No variables set.
            </p>
          ) : (
            <ul className="divide-y rounded-md border">
              {list.map((item) => (
                <li className="flex items-center justify-between gap-3 px-3 py-2" key={item.key}>
                  <span className="min-w-0 truncate font-mono text-sm">{item.key}</span>
                  <span className="shrink-0 font-mono text-muted-foreground text-xs">
                    {item.valueHint}
                  </span>
                </li>
              ))}
            </ul>
          )}
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
        </DialogPanel>
        <DialogFooter>
          {editing ? (
            <>
              <Button
                onClick={() => {
                  setEditing(false);
                  setError(null);
                }}
                type="button"
                variant="outline"
              >
                Cancel
              </Button>
              <Button loading={save.isPending} onClick={() => save.mutate()} type="button">
                Save variables
              </Button>
            </>
          ) : (
            <>
              <DialogClose render={<Button variant="outline">Close</Button>} />
              <Button loading={preparing} onClick={startEdit} type="button">
                <PencilIcon />
                Edit
              </Button>
            </>
          )}
        </DialogFooter>
      </DialogPopup>
    </Dialog>
  );
}

// --- Create / import dialogs ------------------------------------------------

function firstContainerPort(container: ImportableDockerContainer): number {
  return container.ports.find((port) => port.privatePort > 0)?.privatePort ?? 3000;
}

function containerPortsLabel(container: ImportableDockerContainer): string {
  const ports = container.ports
    .filter((port) => port.privatePort > 0)
    .map((port) =>
      port.publicPort
        ? `${port.publicPort}:${port.privatePort}/${port.type}`
        : `${port.privatePort}/${port.type}`,
    );
  return ports.length > 0 ? ports.join(", ") : "No exposed ports";
}

function ImportContainerDialog({ environmentId }: { environmentId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);
  const [serverId, setServerId] = useState("");
  const [containerId, setContainerId] = useState("");
  const [name, setName] = useState("");
  const [port, setPort] = useState("3000");
  const [error, setError] = useState<string | null>(null);

  const servers = useQuery({ queryKey: ["servers", "for-container-import"], queryFn: listServers });
  const activeServers = (servers.data ?? []).filter((server) => server.status === "active");

  useEffect(() => {
    if (open && !serverId && activeServers[0]) {
      setServerId(activeServers[0].id);
    }
  }, [activeServers, open, serverId]);

  const containers = useQuery({
    queryKey: ["importable-containers", serverId],
    queryFn: () => listImportableDockerContainers(serverId),
    enabled: open && Boolean(serverId),
  });
  const containerList = containers.data ?? [];
  const selectedContainer = containerList.find((container) => container.id === containerId);

  function reset() {
    setContainerId("");
    setName("");
    setPort("3000");
    setError(null);
  }

  function selectContainer(container: ImportableDockerContainer) {
    if (!container.running) return;
    setContainerId(container.id);
    setName(container.name);
    setPort(String(firstContainerPort(container)));
  }

  const importMutation = useMutation({
    mutationFn: () =>
      importDockerContainer({
        environmentId,
        serverId,
        containerId,
        name,
        port: Number(port),
      }),
    onSuccess: async () => {
      reset();
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["apps", environmentId] });
      toast.success("Container imported");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function submit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (!serverId) {
      setError("Choose an active server.");
      return;
    }
    if (!containerId) {
      setError("Choose a running container.");
      return;
    }
    if (!name.trim()) {
      setError("Name is required.");
      return;
    }
    const numericPort = Number(port);
    if (!Number.isInteger(numericPort) || numericPort < 1 || numericPort > 65535) {
      setError("Port must be a valid port.");
      return;
    }
    importMutation.mutate();
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        setOpen(next);
        if (!next) reset();
      }}
    >
      <DialogTrigger
        render={
          <Button variant="outline">
            <DownloadIcon />
            Import
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-2xl">
        <form onSubmit={submit}>
          <DialogHeader>
            <DialogTitle>Import container</DialogTitle>
            <DialogDescription>
              Take over a running Docker container that is not already tracked by Basse.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <div className="grid gap-3 sm:grid-cols-[1fr_auto]">
              <div className="space-y-2">
                <Label>Server</Label>
                <Select
                  value={serverId}
                  onValueChange={(value) => {
                    setServerId(value ?? "");
                    setContainerId("");
                  }}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="Server">
                      {(value: string) =>
                        activeServers.find((server) => server.id === value)?.name ?? "Server"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    {activeServers.map((server) => (
                      <SelectItem key={server.id} value={server.id}>
                        {server.name}
                      </SelectItem>
                    ))}
                  </SelectPopup>
                </Select>
              </div>
              <div className="flex items-end">
                <Button
                  disabled={!serverId || containers.isFetching}
                  onClick={() => void containers.refetch()}
                  type="button"
                  variant="outline"
                >
                  <RefreshCwIcon />
                  Scan
                </Button>
              </div>
            </div>
            {servers.isPending ? (
              <p className="text-muted-foreground text-sm">Loading servers…</p>
            ) : activeServers.length === 0 ? (
              <p className="text-muted-foreground text-sm">
                No active servers are ready for container import.
              </p>
            ) : containers.isPending && serverId ? (
              <div className="h-28 animate-pulse rounded-lg border bg-muted/30" aria-hidden />
            ) : containerList.length === 0 && serverId ? (
              <p className="rounded-lg border border-dashed px-3 py-6 text-center text-muted-foreground text-sm">
                No unmanaged containers found on this server.
              </p>
            ) : (
              <div className="max-h-72 overflow-y-auto rounded-lg border">
                {containerList.map((container) => {
                  const selected = container.id === containerId;
                  return (
                    <button
                      className="flex w-full min-w-0 items-center justify-between gap-3 border-b px-3 py-2.5 text-left text-sm last:border-b-0 hover:bg-accent/40 disabled:cursor-not-allowed disabled:opacity-60 data-[selected=true]:bg-accent"
                      data-selected={selected}
                      disabled={!container.running}
                      key={container.id}
                      onClick={() => selectContainer(container)}
                      type="button"
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{container.name}</span>
                        <span className="block truncate font-mono text-muted-foreground text-xs">
                          {container.image}
                        </span>
                      </span>
                      <span className="shrink-0 text-right">
                        <Badge size="sm" variant={container.running ? "outline" : "secondary"}>
                          {container.running ? "running" : container.state}
                        </Badge>
                        <span className="mt-1 block font-mono text-muted-foreground text-xs">
                          {containerPortsLabel(container)}
                        </span>
                      </span>
                    </button>
                  );
                })}
              </div>
            )}
            {selectedContainer ? (
              <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                <div className="space-y-2">
                  <Label htmlFor="import-app-name">App name</Label>
                  <Input
                    id="import-app-name"
                    onChange={(event) => setName(event.currentTarget.value)}
                    value={name}
                  />
                </div>
                <div className="space-y-2">
                  <Label htmlFor="import-app-port">Port</Label>
                  <Input
                    id="import-app-port"
                    onChange={(event) => setPort(event.currentTarget.value)}
                    type="number"
                    value={port}
                  />
                </div>
              </div>
            ) : null}
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button
              disabled={!containerId || !name.trim() || !serverId}
              loading={importMutation.isPending}
              type="submit"
            >
              Import container
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

function NewEnvironmentDialog({
  projectId,
  open,
  onOpenChange,
}: {
  projectId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}) {
  const queryClient = useQueryClient();
  const [name, setName] = useState("");
  const [error, setError] = useState<string | null>(null);

  const add = useMutation({
    mutationFn: () => createEnvironment(projectId, name),
    onSuccess: async () => {
      setName("");
      setError(null);
      onOpenChange(false);
      await queryClient.invalidateQueries({ queryKey: ["environments", projectId] });
      toast.success("Environment created");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next);
        if (!next) setError(null);
      }}
    >
      <DialogPopup className="h-fit max-w-md">
        <form
          onSubmit={(event) => {
            event.preventDefault();
            add.mutate();
          }}
        >
          <DialogHeader>
            <DialogTitle>New environment</DialogTitle>
            <DialogDescription>
              Environments isolate apps and variables — for example staging or preview.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-2">
            <Label htmlFor="environment-name">Name</Label>
            <Input
              autoFocus
              id="environment-name"
              onChange={(event) => setName(event.currentTarget.value)}
              placeholder="staging"
              required
              value={name}
            />
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button disabled={!name.trim()} loading={add.isPending} type="submit">
              Add environment
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}

function CreateAppDialog({ environmentId }: { environmentId: string }) {
  const queryClient = useQueryClient();
  const [open, setOpen] = useState(false);

  const servers = useQuery({ queryKey: ["servers", "for-apps"], queryFn: listServers });
  const githubRepositories = useQuery({
    queryKey: ["github-repositories", "create-app"],
    queryFn: listGitHubRepositories,
  });

  const [name, setName] = useState("");
  const [appKind, setAppKind] = useState<AppKind>("service");
  const [databaseKind, setDatabaseKind] = useState<DatabaseKind>("postgres");
  const [sourceType, setSourceType] = useState<AppSourceType>("repository");
  const [repositoryUrl, setRepositoryUrl] = useState("");
  const [imageRef, setImageRef] = useState("");
  const [branch, setBranch] = useState("main");
  const [port, setPort] = useState("3000");
  const [serverIds, setServerIds] = useState<string[]>([]);
  const [buildMode, setBuildMode] = useState<AppBuildMode>("auto");
  const [buildRootDirectory, setBuildRootDirectory] = useState("");
  const [dockerfilePath, setDockerfilePath] = useState("Dockerfile");
  const [buildRunner, setBuildRunner] = useState<AppBuildRunner>("depot");
  const [databaseVersion, setDatabaseVersion] = useState("18");
  const [databaseName, setDatabaseName] = useState("");
  const [databaseUser, setDatabaseUser] = useState("postgres");
  const [databasePassword, setDatabasePassword] = useState("");
  const [databasePublicEnabled, setDatabasePublicEnabled] = useState(false);
  const [databasePublicPort, setDatabasePublicPort] = useState("5432");
  const [error, setError] = useState<string | null>(null);

  function reset() {
    setName("");
    setAppKind("service");
    setDatabaseKind("postgres");
    setSourceType("repository");
    setRepositoryUrl("");
    setImageRef("");
    setBranch("main");
    setPort("3000");
    setServerIds([]);
    setBuildMode("auto");
    setBuildRootDirectory("");
    setDockerfilePath("Dockerfile");
    setBuildRunner("depot");
    setDatabaseVersion("18");
    setDatabaseName("");
    setDatabaseUser("postgres");
    setDatabasePassword("");
    setDatabasePublicEnabled(false);
    setDatabasePublicPort("5432");
    setError(null);
  }

  const localBuildInvalid =
    appKind === "service" && buildRunner === "server" && serverIds.length !== 1;
  const databaseServerInvalid = appKind === "database" && serverIds.length !== 1;
  const add = useMutation({
    mutationFn: async () => {
      if (appKind === "database") {
        const created = await createApp({
          environmentId,
          name,
          appKind: "database",
          serverIds,
          databaseKind,
          databaseVersion,
          databaseName: databaseKind === "postgres" ? databaseName : undefined,
          databaseUser: databaseKind === "postgres" ? databaseUser : undefined,
          databasePassword: databasePassword || undefined,
          databasePublicEnabled,
          databasePublicPort: databasePublicEnabled ? Number(databasePublicPort) : null,
        });
        await triggerDeploy(created.id);
        return created;
      }

      return createApp({
        environmentId,
        name,
        sourceType,
        repositoryUrl,
        imageRef,
        branch,
        port: Number(port),
        serverIds,
        buildMode,
        buildRootDirectory,
        dockerfilePath,
        buildRunner,
      });
    },
    onSuccess: async () => {
      const kind = appKind;
      reset();
      setOpen(false);
      await queryClient.invalidateQueries({ queryKey: ["apps", environmentId] });
      toast.success(kind === "database" ? "Database created" : "App created");
    },
    onError: (mutationError: Error) => setError(mutationError.message),
  });

  function handleSubmit(event: FormEvent<HTMLFormElement>) {
    event.preventDefault();
    if (localBuildInvalid) {
      setError("Selected-server builds require exactly one server.");
      return;
    }
    if (databaseServerInvalid) {
      setError("Databases require exactly one server.");
      return;
    }
    add.mutate();
  }

  function toggleServer(serverId: string, checked: boolean) {
    setServerIds((current) =>
      appKind === "database"
        ? checked
          ? [serverId]
          : []
        : checked
          ? [...new Set([...current, serverId])]
          : current.filter((selectedServerId) => selectedServerId !== serverId),
    );
  }

  const serverList = servers.data ?? [];
  const githubRepoList = githubRepositories.data?.repositories ?? [];
  const githubRepoErrors = githubRepositories.data?.errors ?? [];

  function updateDatabaseKind(kind: DatabaseKind) {
    setDatabaseKind(kind);
    if (kind === "postgres") {
      setDatabaseVersion("18");
      setDatabaseUser("postgres");
      setDatabasePublicPort("5432");
    } else {
      setDatabaseVersion("8");
      setDatabaseName("");
      setDatabaseUser("");
      setDatabasePublicPort("6379");
    }
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
        render={
          <Button>
            <PlusIcon />
            New app
          </Button>
        }
      />
      <DialogPopup className="h-fit max-w-lg">
        <form onSubmit={handleSubmit}>
          <DialogHeader>
            <DialogTitle>New app</DialogTitle>
            <DialogDescription>
              Deploy an application or create a managed database.
            </DialogDescription>
          </DialogHeader>
          <DialogPanel className="space-y-4">
            <div className="space-y-2">
              <Label htmlFor="app-name">Name</Label>
              <Input
                autoFocus
                id="app-name"
                onChange={(event) => setName(event.currentTarget.value)}
                placeholder="web"
                required
                value={name}
              />
            </div>
            <div className="space-y-2">
              <Label>Type</Label>
              <Select
                value={appKind}
                onValueChange={(value) => {
                  setAppKind((value ?? "service") as AppKind);
                  setServerIds([]);
                }}
              >
                <SelectTrigger className="w-full">
                  <SelectValue placeholder="Type">
                    {(value: AppKind) =>
                      value === "database" ? "Managed database" : "Application"
                    }
                  </SelectValue>
                </SelectTrigger>
                <SelectPopup>
                  <SelectItem value="service">Application</SelectItem>
                  <SelectItem value="database">Managed database</SelectItem>
                </SelectPopup>
              </Select>
            </div>
            {appKind === "service" ? (
              <>
                <div className="space-y-2">
                  <Label>Source</Label>
                  <Select
                    value={sourceType}
                    onValueChange={(value) =>
                      setSourceType((value ?? "repository") as AppSourceType)
                    }
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Source">
                        {(value: AppSourceType) =>
                          value === "image" ? "Prebuilt Docker image" : "Git repository"
                        }
                      </SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      <SelectItem value="repository">Git repository</SelectItem>
                      <SelectItem value="image">Prebuilt Docker image</SelectItem>
                    </SelectPopup>
                  </Select>
                </div>
                {sourceType === "repository" ? (
                  <>
                    {githubRepoList.length > 0 ? (
                      <GitHubRepositorySelect
                        label="Private GitHub repository"
                        onSelect={(repository) => {
                          setRepositoryUrl(repository.cloneUrl);
                          setBranch(repository.defaultBranch);
                        }}
                        repositories={githubRepoList}
                        value={repositoryUrl}
                      />
                    ) : null}
                    {githubRepositories.isError ? (
                      <p className="text-destructive-foreground text-sm">
                        Couldn't load installed GitHub repositories:{" "}
                        {toMessage(githubRepositories.error)}
                      </p>
                    ) : githubRepoErrors.length > 0 ? (
                      <p className="text-muted-foreground text-sm">
                        Some GitHub installations could not be loaded: {githubRepoErrors.join("; ")}
                      </p>
                    ) : !githubRepositories.isPending && githubRepoList.length === 0 ? (
                      <p className="text-muted-foreground text-sm">
                        Need a private repository?{" "}
                        <Link
                          className="underline underline-offset-4"
                          search={{
                            code: undefined,
                            installation_id: undefined,
                            setup_action: undefined,
                            state: undefined,
                          }}
                          to="/secrets"
                        >
                          Install the GitHub App in Secrets
                        </Link>
                        .
                      </p>
                    ) : null}
                    <div className="space-y-2">
                      <Label htmlFor="app-repo">Public or manual repository URL</Label>
                      <Input
                        id="app-repo"
                        onChange={(event) => setRepositoryUrl(event.currentTarget.value)}
                        placeholder="https://github.com/user/repo"
                        required
                        value={repositoryUrl}
                      />
                    </div>
                    <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                      <div className="space-y-2">
                        <Label htmlFor="app-branch">Branch</Label>
                        <Input
                          id="app-branch"
                          onChange={(event) => setBranch(event.currentTarget.value)}
                          value={branch}
                        />
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="app-port">Port</Label>
                        <Input
                          id="app-port"
                          onChange={(event) => setPort(event.currentTarget.value)}
                          type="number"
                          value={port}
                        />
                      </div>
                    </div>
                    <div className="space-y-2">
                      <Label>Build mode</Label>
                      <Select
                        value={buildMode}
                        onValueChange={(value) =>
                          setBuildMode((value ?? "auto") as AppBuildMode)
                        }
                      >
                        <SelectTrigger className="w-full">
                          <SelectValue placeholder="Build mode">
                            {(value: AppBuildMode) =>
                              value === "dockerfile"
                                ? "Force Dockerfile"
                                : value === "railpack"
                                  ? "Force Railpack"
                                  : "Auto detect"
                            }
                          </SelectValue>
                        </SelectTrigger>
                        <SelectPopup>
                          <SelectItem value="auto">Auto detect</SelectItem>
                          <SelectItem value="dockerfile">Force Dockerfile</SelectItem>
                          <SelectItem value="railpack">Force Railpack</SelectItem>
                        </SelectPopup>
                      </Select>
                    </div>
                    <div className="grid gap-3 sm:grid-cols-2">
                      <div className="space-y-2">
                        <Label htmlFor="app-build-root">Root directory</Label>
                        <Input
                          id="app-build-root"
                          onChange={(event) => setBuildRootDirectory(event.currentTarget.value)}
                          placeholder="."
                          value={buildRootDirectory}
                        />
                      </div>
                      <div className="space-y-2">
                        <Label htmlFor="app-dockerfile-path">Dockerfile path</Label>
                        <Input
                          id="app-dockerfile-path"
                          onChange={(event) => setDockerfilePath(event.currentTarget.value)}
                          placeholder="Dockerfile"
                          value={dockerfilePath}
                        />
                      </div>
                    </div>
                  </>
                ) : (
                  <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                    <div className="space-y-2">
                      <Label htmlFor="app-image">Docker image</Label>
                      <Input
                        id="app-image"
                        onChange={(event) => setImageRef(event.currentTarget.value)}
                        placeholder="nginx:alpine"
                        required
                        value={imageRef}
                      />
                    </div>
                    <div className="space-y-2">
                      <Label htmlFor="app-image-port">Port</Label>
                      <Input
                        id="app-image-port"
                        onChange={(event) => setPort(event.currentTarget.value)}
                        type="number"
                        value={port}
                      />
                    </div>
                  </div>
                )}
              </>
            ) : (
              <>
                <div className="space-y-2">
                  <Label>Engine</Label>
                  <Select
                    value={databaseKind}
                    onValueChange={(value) =>
                      updateDatabaseKind((value ?? "postgres") as DatabaseKind)
                    }
                  >
                    <SelectTrigger className="w-full">
                      <SelectValue placeholder="Engine">
                        {(value: DatabaseKind) => (
                          <span className="flex items-center gap-2">
                            <DatabaseIcon className="size-4" kind={value} />
                            <span>{databaseEngineLabel(value)}</span>
                          </span>
                        )}
                      </SelectValue>
                    </SelectTrigger>
                    <SelectPopup>
                      <SelectItem value="postgres">
                        <span className="flex items-center gap-2">
                          <DatabaseIcon className="size-4" kind="postgres" />
                          <span>Postgres</span>
                        </span>
                      </SelectItem>
                      <SelectItem value="redis">
                        <span className="flex items-center gap-2">
                          <DatabaseIcon className="size-4" kind="redis" />
                          <span>Redis</span>
                        </span>
                      </SelectItem>
                    </SelectPopup>
                  </Select>
                </div>
                <div className="grid gap-3 sm:grid-cols-[1fr_120px]">
                  {databaseKind === "postgres" ? (
                    <div className="space-y-2">
                      <Label htmlFor="database-name">Database name</Label>
                      <Input
                        id="database-name"
                        onChange={(event) => setDatabaseName(event.currentTarget.value)}
                        placeholder={name || "app"}
                        value={databaseName}
                      />
                    </div>
                  ) : null}
                  <div className="space-y-2">
                    <Label htmlFor="database-version">Version</Label>
                    <Input
                      id="database-version"
                      onChange={(event) => setDatabaseVersion(event.currentTarget.value)}
                      value={databaseVersion}
                    />
                  </div>
                </div>
                <div className="grid gap-3 sm:grid-cols-2">
                  {databaseKind === "postgres" ? (
                    <div className="space-y-2">
                      <Label htmlFor="database-user">User</Label>
                      <Input
                        id="database-user"
                        onChange={(event) => setDatabaseUser(event.currentTarget.value)}
                        value={databaseUser}
                      />
                    </div>
                  ) : null}
                  <div className="space-y-2">
                    <Label htmlFor="database-password">Password</Label>
                    <Input
                      id="database-password"
                      onChange={(event) => setDatabasePassword(event.currentTarget.value)}
                      placeholder="Generate"
                      type="password"
                      value={databasePassword}
                    />
                  </div>
                </div>
                <label className="flex items-center gap-2 text-sm">
                  <Checkbox
                    checked={databasePublicEnabled}
                    onCheckedChange={(value) => setDatabasePublicEnabled(value === true)}
                  />
                  <span>Enable public TCP access</span>
                </label>
                {databasePublicEnabled ? (
                  <div className="max-w-40 space-y-2">
                    <Label htmlFor="database-public-port">Public port</Label>
                    <Input
                      id="database-public-port"
                      onChange={(event) => setDatabasePublicPort(event.currentTarget.value)}
                      type="number"
                      value={databasePublicPort}
                    />
                  </div>
                ) : null}
              </>
            )}
            <div className="space-y-2">
              <Label>Servers</Label>
              {servers.isPending ? (
                <p className="text-muted-foreground text-sm">Loading servers…</p>
              ) : serverList.length === 0 ? (
                <p className="text-muted-foreground text-sm">
                  No servers yet —{" "}
                  <Link className="font-medium text-foreground underline" to="/servers">
                    add one
                  </Link>{" "}
                  first (you can also attach it later).
                </p>
              ) : (
                <div className="flex flex-col gap-2">
                  {serverList.map((server) => (
                    <label
                      key={server.id}
                      className="flex items-center justify-between gap-3 rounded-md border px-3 py-2 text-sm"
                    >
                      <span className="min-w-0">
                        <span className="block truncate font-medium">{server.name}</span>
                        <span className="text-muted-foreground">{server.status}</span>
                      </span>
                      <Checkbox
                        checked={serverIds.includes(server.id)}
                        onCheckedChange={(value) => toggleServer(server.id, value === true)}
                      />
                    </label>
                  ))}
                </div>
              )}
            </div>
            {appKind === "service" && sourceType === "repository" ? (
              <div className="space-y-2">
                <Label>Build location</Label>
                <Select
                  value={buildRunner}
                  onValueChange={(value) => setBuildRunner((value ?? "depot") as AppBuildRunner)}
                >
                  <SelectTrigger className="w-full">
                    <SelectValue placeholder="Build location">
                      {(value: AppBuildRunner) =>
                        value === "server" ? "Selected server" : "Depot"
                      }
                    </SelectValue>
                  </SelectTrigger>
                  <SelectPopup>
                    <SelectItem value="depot">Depot</SelectItem>
                    <SelectItem value="server">Selected server</SelectItem>
                  </SelectPopup>
                </Select>
                {localBuildInvalid ? (
                  <p className="text-warning-foreground text-sm">
                    Selected-server builds require exactly one server. Use Depot for multiple
                    servers.
                  </p>
                ) : null}
              </div>
            ) : null}
            {databaseServerInvalid ? (
              <p className="text-warning-foreground text-sm">
                Databases require exactly one server.
              </p>
            ) : null}
            {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          </DialogPanel>
          <DialogFooter>
            <DialogClose render={<Button variant="outline">Cancel</Button>} />
            <Button
              disabled={!name.trim() || localBuildInvalid || databaseServerInvalid}
              loading={add.isPending}
              type="submit"
            >
              {appKind === "database" ? "Create database" : "Create app"}
            </Button>
          </DialogFooter>
        </form>
      </DialogPopup>
    </Dialog>
  );
}
