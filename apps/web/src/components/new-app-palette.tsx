import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import {
  ArrowLeftIcon,
  BoxIcon,
  ChevronRightIcon,
  ContainerIcon,
  DownloadIcon,
  GitBranchIcon,
  SearchIcon,
  ServerIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import type { DatabaseKind } from "@basse/shared";
import { DatabaseIcon, NeonIcon, databaseEngineLabel } from "@/components/database-icon";
import { Dialog, DialogPopup, DialogTitle } from "@/components/ui/dialog";
import { Spinner } from "@/components/ui/spinner";
import { createApp } from "@/lib/apps";
import { triggerDeploy } from "@/lib/deployments";
import { listGitHubRepositories } from "@/lib/github";
import { listNeonRegions } from "@/lib/neon";
import { listServers } from "@/lib/servers";
import { toast, toMessage } from "@/lib/toast";
import { cn } from "@/lib/utils";

type PaletteView =
  | "root"
  | "github"
  | "image"
  | "database"
  | "database-server"
  | "neon"
  | "neon-name";

type PaletteRow = {
  key: string;
  icon: ReactNode;
  label: string;
  hint?: string;
  drill?: boolean;
  disabled?: boolean;
  onSelect: () => void;
};

function nameFromRepoUrl(url: string): string {
  const tail = url.replace(/\/+$/, "").split("/").pop() ?? "app";
  return tail.replace(/\.git$/, "") || "app";
}

function nameFromImage(imageRef: string): string {
  const base = imageRef.split("/").pop() ?? imageRef;
  return base.split(":")[0] || "app";
}

/** Railway-style creation flow: one search box, drill into GitHub repos,
 * a Docker image, or a managed database — created with sane defaults, then
 * configured in the app panel. */
export function NewAppPalette({
  environmentId,
  open,
  onOpenChange,
  onCreated,
  onRequestImport,
}: {
  environmentId: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
  onCreated: (appId: string) => void;
  onRequestImport: () => void;
}) {
  const queryClient = useQueryClient();
  const [view, setView] = useState<PaletteView>("root");
  const [query, setQuery] = useState("");
  const [highlight, setHighlight] = useState(0);
  const [databaseKind, setDatabaseKind] = useState<DatabaseKind>("postgres");
  const [neonRegion, setNeonRegion] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement>(null);

  const repos = useQuery({
    queryKey: ["github-repositories", "palette"],
    queryFn: listGitHubRepositories,
    enabled: open,
  });
  const servers = useQuery({
    queryKey: ["servers", "palette"],
    queryFn: listServers,
    enabled: open,
  });
  const neonRegions = useQuery({
    queryKey: ["neon-regions", "palette"],
    queryFn: listNeonRegions,
    enabled: open && view === "neon",
    retry: false,
  });
  const activeServers = (servers.data ?? []).filter((server) => server.status === "active");

  function reset() {
    setView("root");
    setQuery("");
    setHighlight(0);
  }

  const create = useMutation({
    mutationFn: async (input: Parameters<typeof createApp>[0] & { deploy?: boolean }) => {
      const { deploy, ...appInput } = input;
      const created = await createApp(appInput);
      if (deploy) await triggerDeploy(created.id);
      return created;
    },
    onSuccess: async (created) => {
      await queryClient.invalidateQueries({ queryKey: ["apps", environmentId] });
      toast.success(`${created.name} created`);
      onOpenChange(false);
      reset();
      onCreated(created.id);
    },
    onError: (error: Error) =>
      toast.error("Couldn't create app", { description: toMessage(error) }),
  });

  function goTo(next: PaletteView) {
    setView(next);
    setQuery("");
    setHighlight(0);
    requestAnimationFrame(() => inputRef.current?.focus());
  }

  const repoList = repos.data?.repositories ?? [];
  const needle = query.trim().toLowerCase();
  const looksLikeRepoUrl = /^(https?:\/\/|git@)/.test(query.trim());

  const rows = useMemo<PaletteRow[]>(() => {
    if (view === "root") {
      const rootRows: PaletteRow[] = [
        {
          key: "github",
          icon: <GitBranchIcon className="size-4" />,
          label: "GitHub repository",
          drill: true,
          onSelect: () => goTo("github"),
        },
        {
          key: "image",
          icon: <ContainerIcon className="size-4" />,
          label: "Docker image",
          drill: true,
          onSelect: () => goTo("image"),
        },
        {
          key: "database",
          icon: <DatabaseIcon className="size-4" kind="postgres" />,
          label: "Database",
          drill: true,
          onSelect: () => goTo("database"),
        },
        {
          key: "neon",
          icon: <NeonIcon className="size-4" />,
          label: "Neon Postgres",
          drill: true,
          onSelect: () => goTo("neon"),
        },
        {
          key: "import",
          icon: <DownloadIcon className="size-4" />,
          label: "Import running container",
          onSelect: () => {
            onOpenChange(false);
            reset();
            onRequestImport();
          },
        },
      ];
      return needle ? rootRows.filter((row) => row.label.toLowerCase().includes(needle)) : rootRows;
    }

    if (view === "github") {
      const repoRows: PaletteRow[] = repoList
        .filter((repo) => !needle || repo.fullName.toLowerCase().includes(needle))
        .slice(0, 8)
        .map((repo) => ({
          key: repo.id,
          icon: <GitBranchIcon className="size-4" />,
          label: repo.fullName,
          hint: repo.defaultBranch,
          onSelect: () =>
            create.mutate({
              environmentId,
              name: nameFromRepoUrl(repo.cloneUrl),
              sourceType: "repository",
              repositoryUrl: repo.cloneUrl,
              branch: repo.defaultBranch,
              port: 3000,
            }),
        }));
      if (looksLikeRepoUrl) {
        repoRows.unshift({
          key: "__url__",
          icon: <BoxIcon className="size-4" />,
          label: `Deploy ${query.trim()}`,
          hint: "public repo",
          onSelect: () =>
            create.mutate({
              environmentId,
              name: nameFromRepoUrl(query.trim()),
              sourceType: "repository",
              repositoryUrl: query.trim(),
              branch: "main",
              port: 3000,
            }),
        });
      }
      return repoRows;
    }

    if (view === "image") {
      if (!query.trim()) return [];
      return [
        {
          key: "__image__",
          icon: <ContainerIcon className="size-4" />,
          label: `Deploy ${query.trim()}`,
          hint: "docker image",
          onSelect: () =>
            create.mutate({
              environmentId,
              name: nameFromImage(query.trim()),
              sourceType: "image",
              imageRef: query.trim(),
              port: 3000,
            }),
        },
      ];
    }

    if (view === "neon") {
      // Neon databases pick a region, not a server — Neon hosts them.
      return (neonRegions.data ?? [])
        .filter(
          (region) =>
            !needle ||
            region.name.toLowerCase().includes(needle) ||
            region.id.toLowerCase().includes(needle),
        )
        .map((region) => ({
          key: region.id,
          icon: <NeonIcon className="size-4" />,
          label: region.name,
          hint: region.id,
          drill: true,
          onSelect: () => {
            setNeonRegion(region.id);
            goTo("neon-name");
          },
        }));
    }

    if (view === "neon-name") {
      const name = query.trim() || "neon";
      return [
        {
          key: "__neon-name__",
          icon: <NeonIcon className="size-4" />,
          label: `Create ${name}`,
          hint: neonRegion ?? undefined,
          onSelect: () =>
            create.mutate({
              environmentId,
              name,
              appKind: "neon",
              neonRegion: neonRegion ?? undefined,
            }),
        },
      ];
    }

    if (view === "database") {
      const engines: DatabaseKind[] = ["postgres", "redis"];
      return engines
        .filter((kind) => !needle || databaseEngineLabel(kind).toLowerCase().includes(needle))
        .map((kind) => ({
          key: kind,
          icon: <DatabaseIcon className="size-4" kind={kind} />,
          label: databaseEngineLabel(kind),
          drill: true,
          onSelect: () => {
            setDatabaseKind(kind);
            goTo("database-server");
          },
        }));
    }

    // database-server: databases deploy to exactly one server.
    return activeServers
      .filter((server) => !needle || server.name.toLowerCase().includes(needle))
      .map((server) => ({
        key: server.id,
        icon: <ServerIcon className="size-4" />,
        label: server.name,
        hint: "deploy here",
        onSelect: () =>
          create.mutate({
            environmentId,
            name: databaseKind,
            appKind: "database",
            serverIds: [server.id],
            databaseKind,
            databaseVersion: databaseKind === "postgres" ? "18" : "8",
            databaseName: databaseKind === "postgres" ? databaseKind : undefined,
            databaseUser: databaseKind === "postgres" ? "postgres" : undefined,
            deploy: true,
          }),
      }));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [
    view,
    needle,
    repoList,
    activeServers,
    databaseKind,
    looksLikeRepoUrl,
    query,
    neonRegions.data,
    neonRegion,
  ]);

  useEffect(() => {
    setHighlight((current) => Math.min(current, Math.max(rows.length - 1, 0)));
  }, [rows.length]);

  const placeholder =
    view === "root"
      ? "What would you like to create?"
      : view === "github"
        ? "Search repositories or paste a URL"
        : view === "image"
          ? "nginx:alpine, ghcr.io/acme/api:latest…"
          : view === "database"
            ? "Postgres or Redis"
            : view === "neon"
              ? "Pick a region"
              : view === "neon-name"
                ? "Database name (defaults to neon)"
                : "Pick a server";

  const emptyText =
    view === "github"
      ? repos.isPending
        ? null
        : repoList.length === 0
          ? "no-github"
          : "No repositories match."
      : view === "image"
        ? "Type a Docker image reference and press enter."
        : view === "database-server" && activeServers.length === 0
          ? "no-servers"
          : view === "neon"
            ? neonRegions.isPending
              ? null
              : neonRegions.isError
                ? "no-neon"
                : "No regions match."
            : "Nothing matches.";

  function handleKeyDown(event: React.KeyboardEvent<HTMLInputElement>) {
    if (event.key === "ArrowDown") {
      event.preventDefault();
      setHighlight((current) => Math.min(current + 1, rows.length - 1));
    } else if (event.key === "ArrowUp") {
      event.preventDefault();
      setHighlight((current) => Math.max(current - 1, 0));
    } else if (event.key === "Enter") {
      event.preventDefault();
      rows[highlight]?.onSelect();
    } else if (event.key === "Backspace" && query === "" && view !== "root") {
      event.preventDefault();
      goTo(view === "database-server" ? "database" : view === "neon-name" ? "neon" : "root");
    }
  }

  return (
    <Dialog
      open={open}
      onOpenChange={(next) => {
        onOpenChange(next);
        if (!next) reset();
      }}
    >
      <DialogPopup className="h-fit max-w-xl overflow-hidden p-0">
        <DialogTitle className="sr-only">New app</DialogTitle>
        <div className="flex items-center gap-2.5 border-b px-3.5 py-3">
          {view !== "root" ? (
            <button
              aria-label="Back"
              className="text-muted-foreground transition hover:text-foreground"
              onClick={() =>
                goTo(
                  view === "database-server" ? "database" : view === "neon-name" ? "neon" : "root",
                )
              }
              type="button"
            >
              <ArrowLeftIcon className="size-4" />
            </button>
          ) : (
            <SearchIcon className="size-4 text-muted-foreground" />
          )}
          <input
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
            onChange={(event) => {
              setQuery(event.currentTarget.value);
              setHighlight(0);
            }}
            onKeyDown={handleKeyDown}
            placeholder={placeholder}
            ref={inputRef}
            spellCheck={false}
            value={query}
          />
          {create.isPending ||
          (view === "github" && repos.isPending) ||
          (view === "neon" && neonRegions.isPending) ? (
            <Spinner className="size-4 text-muted-foreground" />
          ) : null}
        </div>

        <div className="max-h-80 overflow-y-auto p-1.5">
          {rows.length > 0 ? (
            rows.map((row, index) => (
              <button
                className={cn(
                  "flex w-full items-center gap-3 rounded-lg px-3 py-2 text-left text-sm outline-none transition",
                  index === highlight ? "bg-accent text-accent-foreground" : "hover:bg-accent/50",
                  create.isPending && "pointer-events-none opacity-60",
                )}
                key={row.key}
                onClick={row.onSelect}
                onMouseEnter={() => setHighlight(index)}
                type="button"
              >
                <span className="text-muted-foreground">{row.icon}</span>
                <span className="min-w-0 flex-1 truncate">{row.label}</span>
                {row.hint ? (
                  <span className="shrink-0 font-mono text-muted-foreground text-xs">
                    {row.hint}
                  </span>
                ) : null}
                {row.drill ? (
                  <ChevronRightIcon className="size-3.5 shrink-0 text-muted-foreground/60" />
                ) : null}
              </button>
            ))
          ) : emptyText === "no-github" ? (
            <p className="px-3 py-8 text-center text-muted-foreground text-sm">
              No GitHub repositories available.{" "}
              <Link
                className="text-foreground underline underline-offset-4"
                search={{
                  code: undefined,
                  installation_id: undefined,
                  setup_action: undefined,
                  state: undefined,
                }}
                to="/secrets"
              >
                Install the GitHub App
              </Link>{" "}
              or paste a public repo URL above.
            </p>
          ) : emptyText === "no-neon" ? (
            <p className="px-3 py-8 text-center text-muted-foreground text-sm">
              {toMessage(neonRegions.error)}{" "}
              <Link
                className="text-foreground underline underline-offset-4"
                search={{
                  code: undefined,
                  installation_id: undefined,
                  setup_action: undefined,
                  state: undefined,
                }}
                to="/secrets"
              >
                Manage integrations
              </Link>
              .
            </p>
          ) : emptyText === "no-servers" ? (
            <p className="px-3 py-8 text-center text-muted-foreground text-sm">
              Databases need an active server.{" "}
              <Link className="text-foreground underline underline-offset-4" to="/servers">
                Connect one first
              </Link>
              .
            </p>
          ) : emptyText ? (
            <p className="px-3 py-8 text-center text-muted-foreground text-sm">{emptyText}</p>
          ) : (
            <p className="px-3 py-8 text-center text-muted-foreground text-sm">Loading…</p>
          )}
        </div>
      </DialogPopup>
    </Dialog>
  );
}
