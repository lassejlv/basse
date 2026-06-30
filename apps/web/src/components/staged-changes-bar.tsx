import { useMutation, useQueryClient } from "@tanstack/react-query";
import { ChevronDownIcon, ChevronUpIcon, RocketIcon, XIcon } from "lucide-react";
import { type ReactNode, useState } from "react";
import type { AppStagedChanges, StagedChange } from "@basse/shared";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { applyChanges, discardChange, discardChanges } from "@/lib/changes";

const APP_FIELD_LABELS: Record<string, string> = {
  name: "Name",
  repositoryUrl: "Repository",
  branch: "Branch",
  port: "Port",
  sourceType: "Source",
  imageRef: "Image",
  buildMode: "Build mode",
  buildRunner: "Build location",
  serverIds: "Servers",
  volumes: "Volumes",
  cpuLimitMillicores: "CPU limit",
  memoryLimitBytes: "Memory limit",
  databaseVersion: "Database version",
  databasePublicEnabled: "Public access",
  databasePublicPort: "Public port",
};

// "slug" is derived from "name", so it is hidden from the diff to avoid noise.
function isHidden(change: StagedChange): boolean {
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
  if (field === "databasePublicEnabled") return value ? "enabled" : "disabled";
  return String(value);
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
  let label: string;
  let detail: ReactNode;

  if (change.resource === "env_var") {
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
    <li className="flex items-center justify-between gap-3 px-3 py-2 text-sm">
      <span className="flex min-w-0 flex-wrap items-baseline gap-x-2">
        <span className="font-medium">{label}</span>
        {detail}
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

export function StagedChangesBar({ appId, changes }: { appId: string; changes: StagedChange[] }) {
  const queryClient = useQueryClient();
  const [expanded, setExpanded] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  const visible = changes.filter((change) => !isHidden(change));

  function cacheChanges(data: AppStagedChanges) {
    queryClient.setQueryData(["changes", appId], data);
  }

  const apply = useMutation({
    mutationFn: () => applyChanges(appId),
    onSuccess: async (result) => {
      setError(null);
      setNotice(
        result.deployment ? null : "Changes saved. Attach a server to this app to deploy them.",
      );
      await Promise.all([
        queryClient.invalidateQueries({ queryKey: ["changes", appId] }),
        queryClient.invalidateQueries({ queryKey: ["app", appId] }),
        queryClient.invalidateQueries({ queryKey: ["deployments", appId] }),
        queryClient.invalidateQueries({ queryKey: ["env-vars", appId] }),
        queryClient.invalidateQueries({ queryKey: ["env-vars-reveal", appId] }),
      ]);
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardAll = useMutation({
    mutationFn: () => discardChanges(appId),
    onSuccess: (data) => {
      setError(null);
      setNotice(null);
      setExpanded(false);
      cacheChanges(data);
    },
    onError: (e: Error) => setError(e.message),
  });

  const discardOne = useMutation({
    mutationFn: (changeId: string) => discardChange(appId, changeId),
    onSuccess: (data) => {
      setError(null);
      cacheChanges(data);
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
              <RocketIcon />
              Deploy
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
