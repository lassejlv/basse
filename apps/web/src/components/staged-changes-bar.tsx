import { useMutation, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, ChevronDownIcon, ChevronUpIcon, RocketIcon, XIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import type {
  AppStagedChanges,
  ProjectStagedChange,
  ProjectStagedChangeHistoryEntry,
  ProjectStagedChanges,
  StagedChange,
  StagedChangeHistoryEntry,
  StagedChangeHistoryItem,
} from "@basse/shared";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import {
  applyChanges,
  applyProjectChanges,
  discardChange,
  discardChanges,
  discardProjectChange,
  discardProjectChanges,
} from "@/lib/changes";

const APP_FIELD_LABELS: Record<string, string> = {
  name: "Name",
  repositoryUrl: "Repository",
  branch: "Branch",
  port: "Port",
  sourceType: "Source",
  imageRef: "Image",
  buildMode: "Build mode",
  buildRootDirectory: "Root directory",
  dockerfilePath: "Dockerfile path",
  buildRunner: "Build location",
  autoRedeployEnabled: "Auto redeploy",
  serverIds: "Servers",
  volumes: "Volumes",
  cpuLimitMillicores: "CPU limit",
  memoryLimitBytes: "Memory limit",
  databaseVersion: "Database version",
  databasePublicEnabled: "Public access",
  databasePublicPort: "Public port",
};

type DisplayChange = Pick<
  StagedChange,
  "resource" | "action" | "field" | "value" | "previousValue"
>;

type DomainChangePayload = {
  host?: string;
  upstream?: string;
};

// "slug" is derived from "name", so it is hidden from the diff to avoid noise.
function isHidden(change: DisplayChange): boolean {
  return change.resource === "app" && change.field === "slug";
}

function formatAppValue(field: string, raw: string | null): string {
  if (raw === null) return "—";
  let value: unknown;
  try {
    value = JSON.parse(raw);
  } catch {
    return raw;
  }
  if (value === null || value === "") return "—";
  if (field === "serverIds") {
    const count = Array.isArray(value) ? value.length : 0;
    return `${count} server${count === 1 ? "" : "s"}`;
  }
  if (field === "volumes") {
    try {
      const list = JSON.parse(value as string) as unknown[];
      return `${list.length} mount${list.length === 1 ? "" : "s"}`;
    } catch {
      return "changed";
    }
  }
  if (field === "cpuLimitMillicores") {
    return typeof value === "number" ? `${value / 1000} cores` : "host default";
  }
  if (field === "memoryLimitBytes") {
    return typeof value === "number" ? `${Math.round(value / 1048576)} MB` : "host default";
  }
  if (field === "buildMode") {
    if (value === "dockerfile") return "Dockerfile";
    if (value === "railpack") return "Railpack";
    return "auto";
  }
  if (field === "buildRootDirectory") {
    return typeof value === "string" && value ? value : ".";
  }
  if (field === "dockerfilePath") {
    return typeof value === "string" && value ? value : "Dockerfile";
  }
  if (field === "autoRedeployEnabled") return value ? "enabled" : "disabled";
  if (field === "databasePublicEnabled") return value ? "enabled" : "disabled";
  return String(value);
}

function parseDomainChangeValue(raw: string | null): DomainChangePayload | null {
  if (!raw) return null;
  try {
    return JSON.parse(raw) as DomainChangePayload;
  } catch {
    return null;
  }
}

function formatDomainHost(change: DisplayChange): string {
  const parsed = parseDomainChangeValue(change.value ?? change.previousValue);
  return parsed?.host ?? change.field.split(":").slice(1).join(":") ?? change.field;
}

function ChangeSummary({ change }: { change: DisplayChange }) {
  let label: string;
  let detail: ReactNode;

  if (change.resource === "domain") {
    label = "Domain";
    const verb =
      change.action === "create" ? "add" : change.action === "delete" ? "remove" : "change";
    detail = (
      <span className="text-muted-foreground">
        {verb} <span className="font-mono text-foreground/80">{formatDomainHost(change)}</span>
      </span>
    );
  } else if (change.resource === "env_var") {
    label = change.field;
    const verb =
      change.action === "create" ? "added" : change.action === "delete" ? "removed" : "changed";
    detail = (
      <span className="text-muted-foreground">
        variable {verb}
        {change.action !== "delete" && change.value ? (
          <span className="ml-1 font-mono text-foreground/80">{change.value}</span>
        ) : null}
      </span>
    );
  } else {
    label = APP_FIELD_LABELS[change.field] ?? change.field;
    detail = (
      <span className="text-muted-foreground">
        <span className="font-mono line-through opacity-70">
          {formatAppValue(change.field, change.previousValue)}
        </span>
        <span className="mx-1.5">→</span>
        <span className="font-mono text-foreground">
          {formatAppValue(change.field, change.value)}
        </span>
      </span>
    );
  }

  return (
    <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
      <span className="font-medium">{label}</span>
      {detail}
    </span>
  );
}

function changeLabel(change: DisplayChange): string {
  if (change.resource === "domain") return `domain ${formatDomainHost(change)}`;
  return change.resource === "env_var"
    ? change.field
    : APP_FIELD_LABELS[change.field] ?? change.field;
}

function hasDeployableChanges(changes: DisplayChange[]): boolean {
  return changes.some((change) => change.resource !== "domain");
}

function ChangeRow({
  change,
  onDiscard,
  busy,
}: {
  change: StagedChange;
  onDiscard: () => void;
  busy: boolean;
}) {
  const label = changeLabel(change);

  return (
    <li className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
      <ChangeSummary change={change} />
      <Button
        aria-label={`Discard ${label} change`}
        className="shrink-0"
        disabled={busy}
        onClick={onDiscard}
        size="icon-xs"
        variant="ghost"
      >
        <XIcon />
      </Button>
    </li>
  );
}

export function StagedChangesBar({ appId, changes }: { appId: string; changes: StagedChange[] }) {
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const visible = changes.filter((change) => !isHidden(change));
  const deployableChanges = hasDeployableChanges(visible);

  function cacheChanges(data: AppStagedChanges) {
    queryClient.setQueryData(["changes", appId], data);
  }

  const apply = useMutation({
    mutationFn: () => applyChanges(appId),
    onSuccess: async (result) => {
      setError(null);
      setNotice(
        result.deployment
          ? null
          : result.domainSyncs > 0
            ? "Domain changes applied. Proxy sync queued."
            : "Changes saved. Attach a server to this app to deploy them.",
      );
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["changes", appId] }),
        queryClient.invalidateQueries({ queryKey: ["change-history", appId] }),
        queryClient.invalidateQueries({ queryKey: ["app", appId] }),
        queryClient.invalidateQueries({ queryKey: ["deployments", appId] }),
        queryClient.invalidateQueries({ queryKey: ["domains"] }),
        queryClient.invalidateQueries({ queryKey: ["env-vars", appId] }),
        queryClient.invalidateQueries({ queryKey: ["env-vars-reveal", appId] }),
      ]);
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardAll = useMutation({
    mutationFn: () => discardChanges(appId),
    onSuccess: async (data) => {
      setError(null);
      setNotice(null);
      setExpanded(false);
      cacheChanges(data);
      await queryClient.invalidateQueries({ queryKey: ["change-history", appId] });
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardOne = useMutation({
    mutationFn: (changeId: string) => discardChange(appId, changeId),
    onSuccess: async (data) => {
      setError(null);
      cacheChanges(data);
      await queryClient.invalidateQueries({ queryKey: ["change-history", appId] });
    },
    onError: (e: Error) => setError(e.message),
  });

  if (visible.length === 0) return null;

  const busy = apply.isPending || discardAll.isPending || discardOne.isPending;

  return (
    <div className="sticky bottom-4 z-20 mt-2">
      <Card className="gap-0 overflow-hidden border-primary/40 p-0 shadow-lg">
        {expanded ? (
          <ul className="max-h-64 divide-y overflow-y-auto border-b">
            {visible.map((change) => (
              <ChangeRow
                key={change.id}
                busy={busy}
                change={change}
                onDiscard={() => discardOne.mutate(change.id)}
              />
            ))}
          </ul>
        ) : null}

        <div className="flex flex-wrap items-center justify-between gap-3 p-3">
          <button
            className="flex cursor-pointer items-center gap-2 text-sm"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            <span className="flex size-6 items-center justify-center rounded-full bg-primary/15 font-semibold text-primary text-xs">
              {visible.length}
            </span>
            <span className="font-medium">
              {visible.length === 1 ? "unsaved change" : "unsaved changes"}
            </span>
            {expanded ? (
              <ChevronDownIcon className="size-4 text-muted-foreground" />
            ) : (
              <ChevronUpIcon className="size-4 text-muted-foreground" />
            )}
          </button>

          <div className="flex items-center gap-2">
            <Button
              disabled={apply.isPending}
              loading={discardAll.isPending}
              onClick={() => discardAll.mutate()}
              size="sm"
              variant="ghost"
            >
              Discard all
            </Button>
            <Button
              disabled={discardAll.isPending}
              loading={apply.isPending}
              onClick={() => apply.mutate()}
              size="sm"
            >
              {deployableChanges ? <RocketIcon /> : <CheckIcon />}
              {deployableChanges ? "Deploy" : "Apply"}
            </Button>
          </div>
        </div>

        {error ? (
          <p className="border-t px-3 py-2 text-destructive-foreground text-sm">{error}</p>
        ) : notice ? (
          <p className="border-t px-3 py-2 text-muted-foreground text-sm">{notice}</p>
        ) : null}
      </Card>
    </div>
  );
}

function ProjectChangeRow({
  change,
  onDiscard,
  busy,
}: {
  change: ProjectStagedChange;
  onDiscard: () => void;
  busy: boolean;
}) {
  const label = changeLabel(change);

  return (
    <li className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
      <span className="min-w-0">
        <span className="mb-1 block truncate text-muted-foreground text-xs">
          {change.environmentName} / {change.appName}
        </span>
        <ChangeSummary change={change} />
      </span>
      <Button
        aria-label={`Discard ${label} change`}
        className="shrink-0"
        disabled={busy}
        onClick={onDiscard}
        size="icon-xs"
        variant="ghost"
      >
        <XIcon />
      </Button>
    </li>
  );
}

export function ProjectStagedChangesBar({
  projectId,
  changes,
}: {
  projectId: string;
  changes: ProjectStagedChange[];
}) {
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);
  const visible = changes.filter((change) => !isHidden(change));
  const deployableChanges = hasDeployableChanges(visible);

  function cacheChanges(data: ProjectStagedChanges) {
    queryClient.setQueryData(["project-changes", projectId], data);
  }

  async function invalidateProjectChanges() {
    await Promise.all([
      queryClient.invalidateQueries({ queryKey: ["project-changes", projectId] }),
      queryClient.invalidateQueries({ queryKey: ["project-change-history", projectId] }),
      queryClient.invalidateQueries({ queryKey: ["changes"] }),
      queryClient.invalidateQueries({ queryKey: ["change-history"] }),
      queryClient.invalidateQueries({ queryKey: ["app"] }),
      queryClient.invalidateQueries({ queryKey: ["deployments"] }),
      queryClient.invalidateQueries({ queryKey: ["domains"] }),
      queryClient.invalidateQueries({ queryKey: ["env-vars"] }),
      queryClient.invalidateQueries({ queryKey: ["env-vars-reveal"] }),
    ]);
  }

  const apply = useMutation({
    mutationFn: () => applyProjectChanges(projectId),
    onSuccess: async (result) => {
      setError(null);
      const domainSyncs = result.deployments.reduce((total, item) => total + item.domainSyncs, 0);
      setNotice(
        result.deployments.some((item) => item.deployment === null && item.domainSyncs === 0)
          ? "Some changes were saved without deploys because those apps have no deploy target."
          : domainSyncs > 0 && result.deployments.every((item) => item.deployment === null)
            ? "Domain changes applied. Proxy sync queued."
          : null,
      );
      await invalidateProjectChanges();
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardAll = useMutation({
    mutationFn: () => discardProjectChanges(projectId),
    onSuccess: async (data) => {
      setError(null);
      setNotice(null);
      setExpanded(false);
      cacheChanges(data);
      await invalidateProjectChanges();
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardOne = useMutation({
    mutationFn: (changeId: string) => discardProjectChange(projectId, changeId),
    onSuccess: async (data) => {
      setError(null);
      cacheChanges(data);
      await invalidateProjectChanges();
    },
    onError: (e: Error) => setError(e.message),
  });

  if (visible.length === 0) return null;

  const busy = apply.isPending || discardAll.isPending || discardOne.isPending;

  return (
    <div className="sticky bottom-4 z-20 mt-2">
      <Card className="gap-0 overflow-hidden border-primary/40 p-0 shadow-lg">
        {expanded ? (
          <ul className="max-h-72 divide-y overflow-y-auto border-b">
            {visible.map((change) => (
              <ProjectChangeRow
                key={change.id}
                busy={busy}
                change={change}
                onDiscard={() => discardOne.mutate(change.id)}
              />
            ))}
          </ul>
        ) : null}

        <div className="flex flex-wrap items-center justify-between gap-3 p-3">
          <button
            className="flex cursor-pointer items-center gap-2 text-sm"
            onClick={() => setExpanded((value) => !value)}
            type="button"
          >
            <span className="flex size-6 items-center justify-center rounded-full bg-primary/15 font-semibold text-primary text-xs">
              {visible.length}
            </span>
            <span className="font-medium">
              {visible.length === 1 ? "project change" : "project changes"}
            </span>
            {expanded ? (
              <ChevronDownIcon className="size-4 text-muted-foreground" />
            ) : (
              <ChevronUpIcon className="size-4 text-muted-foreground" />
            )}
          </button>

          <div className="flex items-center gap-2">
            <Button
              disabled={apply.isPending}
              loading={discardAll.isPending}
              onClick={() => discardAll.mutate()}
              size="sm"
              variant="ghost"
            >
              Discard all
            </Button>
            <Button
              disabled={discardAll.isPending}
              loading={apply.isPending}
              onClick={() => apply.mutate()}
              size="sm"
            >
              {deployableChanges ? <RocketIcon /> : <CheckIcon />}
              {deployableChanges ? "Deploy project" : "Apply project"}
            </Button>
          </div>
        </div>

        {error ? (
          <p className="border-t px-3 py-2 text-destructive-foreground text-sm">{error}</p>
        ) : notice ? (
          <p className="border-t px-3 py-2 text-muted-foreground text-sm">{notice}</p>
        ) : null}
      </Card>
    </div>
  );
}

function historyTitle(entry: StagedChangeHistoryEntry): string {
  const noun = entry.changes.length === 1 ? "change" : "changes";
  return `${entry.outcome === "applied" ? "Applied" : "Discarded"} ${entry.changes.length} ${noun}`;
}

function HistoryChangeRow({ change }: { change: StagedChangeHistoryItem }) {
  return (
    <li className="px-3 py-2 text-sm">
      <ChangeSummary change={change} />
    </li>
  );
}

export function StagedChangesHistory({
  entries,
  isPending,
}: {
  entries: StagedChangeHistoryEntry[];
  isPending: boolean;
}) {
  if (isPending) {
    return (
      <Card className="p-6">
        <h2 className="font-semibold text-lg">Change history</h2>
        <p className="mt-2 text-muted-foreground text-sm">Loading change history…</p>
      </Card>
    );
  }

  if (entries.length === 0) {
    return (
      <Card className="p-6">
        <h2 className="font-semibold text-lg">Change history</h2>
        <p className="mt-2 text-muted-foreground text-sm">
          Applied and discarded staged changes will show up here.
        </p>
      </Card>
    );
  }

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Change history</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Applied and discarded staged changes for this app.
          </p>
        </div>
        <span className="rounded-md border px-2 py-1 text-muted-foreground text-xs">
          Latest {entries.length}
        </span>
      </div>

      <div className="mt-5 flex flex-col gap-3">
        {entries.map((entry) => {
          const visible = entry.changes.filter((change) => !isHidden(change));
          return (
            <div className="rounded-md border" key={entry.id}>
              <div className="flex flex-wrap items-center justify-between gap-2 border-b px-3 py-2">
                <div>
                  <p className="font-medium text-sm">{historyTitle(entry)}</p>
                  <p className="text-muted-foreground text-xs">
                    {new Date(entry.createdAt).toLocaleString()}
                    {entry.deploymentId ? ` · deployment ${entry.deploymentId.slice(0, 8)}` : ""}
                  </p>
                </div>
                <span className="rounded-md bg-muted px-2 py-1 text-muted-foreground text-xs">
                  {entry.outcome}
                </span>
              </div>
              {visible.length > 0 ? (
                <ul className="divide-y">
                  {visible.map((change) => (
                    <HistoryChangeRow change={change} key={change.id} />
                  ))}
                </ul>
              ) : (
                <p className="px-3 py-2 text-muted-foreground text-sm">
                  Only derived changes were recorded.
                </p>
              )}
            </div>
          );
        })}
      </div>
    </Card>
  );
}

export function ProjectStagedChangesHistory({
  entries,
  isPending,
}: {
  entries: ProjectStagedChangeHistoryEntry[];
  isPending: boolean;
}) {
  if (isPending) {
    return (
      <Card className="p-6">
        <h2 className="font-semibold text-lg">Project change history</h2>
        <p className="mt-2 text-muted-foreground text-sm">Loading change history…</p>
      </Card>
    );
  }

  if (entries.length === 0) {
    return (
      <Card className="p-6">
        <h2 className="font-semibold text-lg">Project change history</h2>
        <p className="mt-2 text-muted-foreground text-sm">
          Applied and discarded staged changes from this project will show up here.
        </p>
      </Card>
    );
  }

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Project change history</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Applied and discarded staged changes across this project.
          </p>
        </div>
        <span className="rounded-md border px-2 py-1 text-muted-foreground text-xs">
          Latest {entries.length}
        </span>
      </div>

      <div className="mt-5 flex flex-col gap-3">
        {entries.map((entry) => {
          const visible = entry.changes.filter((change) => !isHidden(change));
          return (
            <div className="rounded-md border" key={entry.id}>
              <div className="flex flex-wrap items-center justify-between gap-2 border-b px-3 py-2">
                <div>
                  <p className="font-medium text-sm">{historyTitle(entry)}</p>
                  <p className="text-muted-foreground text-xs">
                    {entry.environmentName} / {entry.appName} ·{" "}
                    {new Date(entry.createdAt).toLocaleString()}
                    {entry.deploymentId ? ` · deployment ${entry.deploymentId.slice(0, 8)}` : ""}
                  </p>
                </div>
                <span className="rounded-md bg-muted px-2 py-1 text-muted-foreground text-xs">
                  {entry.outcome}
                </span>
              </div>
              {visible.length > 0 ? (
                <ul className="divide-y">
                  {visible.map((change) => (
                    <HistoryChangeRow change={change} key={change.id} />
                  ))}
                </ul>
              ) : (
                <p className="px-3 py-2 text-muted-foreground text-sm">
                  Only derived changes were recorded.
                </p>
              )}
            </div>
          );
        })}
      </div>
    </Card>
  );
}
