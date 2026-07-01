import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import {
  ArchiveRestoreIcon,
  CloudUploadIcon,
  DownloadIcon,
  Loader2Icon,
  PlusIcon,
  Trash2Icon,
} from "lucide-react";
import { useEffect, useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import {
  Select,
  SelectItem,
  SelectPopup,
  SelectTrigger,
  SelectValue,
} from "@/components/ui/select";
import { Switch } from "@/components/ui/switch";
import type { App } from "@/lib/apps";
import {
  backupDownloadUrl,
  createBackup,
  type DatabaseBackup,
  deleteBackup,
  listBackups,
  restoreBackup,
  updateBackupSettings,
} from "@/lib/backups";
import { formatBytes, relativeTime } from "@/lib/format";
import { listS3Connections, uploadBackupToS3 } from "@/lib/s3";
import { toast, toMessage } from "@/lib/toast";

export function BackupsTab({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const backupsQuery = useQuery({
    queryKey: ["backups", app.id],
    queryFn: () => listBackups(app.id),
    refetchInterval: (query) =>
      query.state.data?.backups.some(
        (b) => b.status === "queued" || b.status === "running" || b.s3Status === "uploading",
      )
        ? 4000
        : false,
  });
  const s3Connections = useQuery({
    queryKey: ["s3-connections"],
    queryFn: listS3Connections,
  });

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["backups", app.id] });

  const create = useMutation({
    mutationFn: () => createBackup(app.id),
    onSuccess: async () => {
      toast.success("Backup started");
      await invalidate();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  const backups = backupsQuery.data?.backups ?? [];

  return (
    <div className="flex flex-col gap-4">
      <Card className="p-6">
        <div className="flex items-start justify-between gap-3">
          <div>
            <h2 className="font-semibold text-lg">Backups</h2>
            <p className="mt-1 text-muted-foreground text-sm">
              pg_dump snapshots stored on the database&apos;s server volume.
            </p>
          </div>
          <Button disabled={create.isPending} onClick={() => create.mutate()} size="sm">
            {create.isPending ? <Loader2Icon className="animate-spin" /> : <PlusIcon />}
            Backup now
          </Button>
        </div>
        <div className="mt-4 flex flex-col gap-2">
          {backupsQuery.isPending ? (
            <p className="text-muted-foreground text-sm">Loading backups…</p>
          ) : backupsQuery.isError ? (
            <p className="text-destructive-foreground text-sm">{toMessage(backupsQuery.error)}</p>
          ) : backups.length === 0 ? (
            <p className="text-muted-foreground text-sm">
              No backups yet. Run one now or enable the schedule below.
            </p>
          ) : (
            backups.map((backup) => (
              <BackupRow
                app={app}
                backup={backup}
                hasS3={Boolean(backupsQuery.data?.settings.s3ConnectionId)}
                key={backup.id}
                onChanged={invalidate}
              />
            ))
          )}
        </div>
      </Card>
      {backupsQuery.data ? (
        <BackupScheduleCard
          appId={app.id}
          onSaved={invalidate}
          s3Connections={s3Connections.data ?? []}
          settings={backupsQuery.data.settings}
        />
      ) : null}
    </div>
  );
}

function BackupRow({
  app,
  backup,
  hasS3,
  onChanged,
}: {
  app: App;
  backup: DatabaseBackup;
  hasS3: boolean;
  onChanged: () => void;
}) {
  const upload = useMutation({
    mutationFn: () => uploadBackupToS3(app.id, backup.id),
    onSuccess: () => {
      toast.success("Upload started");
      onChanged();
    },
    onError: (error) => toast.error(toMessage(error)),
  });
  const restore = useMutation({
    mutationFn: () => restoreBackup(app.id, backup.id),
    onSuccess: () => toast.success("Backup restored"),
    onError: (error) => toast.error(toMessage(error)),
  });
  const remove = useMutation({
    mutationFn: () => deleteBackup(app.id, backup.id),
    onSuccess: () => {
      toast.success("Backup deleted");
      onChanged();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  function confirmRestore() {
    if (
      window.confirm(
        `Restore this backup into ${app.database?.name ?? "the database"}? Existing data will be replaced.`,
      )
    ) {
      restore.mutate();
    }
  }

  function confirmDelete() {
    if (window.confirm("Delete this backup? The dump file is removed from the server.")) {
      remove.mutate();
    }
  }

  const busy = restore.isPending || remove.isPending;

  return (
    <div className="flex items-center gap-3 rounded-lg border px-3 py-2">
      <BackupStatusBadge status={backup.status} />
      <div className="min-w-0 flex-1">
        <p className="truncate font-mono text-xs">
          {new Date(backup.createdAt).toLocaleString()}
          <span className="ml-2 text-muted-foreground">{relativeTime(backup.createdAt)}</span>
        </p>
        <p className="mt-0.5 text-muted-foreground text-xs">
          {backup.trigger === "scheduled" ? "Scheduled" : "Manual"}
          {backup.sizeBytes != null ? ` · ${formatBytes(backup.sizeBytes)}` : ""}
          {backup.status === "failed" && backup.error ? ` · ${backup.error}` : ""}
          {backup.s3Status === "failed" && backup.s3Error ? ` · S3: ${backup.s3Error}` : ""}
        </p>
      </div>
      {backup.s3Status === "uploaded" ? (
        <Badge variant="outline">
          <CloudUploadIcon className="size-3" />
          S3
        </Badge>
      ) : backup.s3Status === "uploading" ? (
        <Badge variant="secondary">
          <Loader2Icon className="size-3 animate-spin" />
          S3
        </Badge>
      ) : null}
      {backup.status === "completed" ? (
        <div className="flex shrink-0 items-center gap-1">
          {hasS3 && backup.s3Status !== "uploaded" && backup.s3Status !== "uploading" ? (
            <Button
              aria-label="Upload to S3"
              disabled={upload.isPending}
              onClick={() => upload.mutate()}
              size="icon-sm"
              variant="ghost"
            >
              {upload.isPending ? <Loader2Icon className="animate-spin" /> : <CloudUploadIcon />}
            </Button>
          ) : null}
          <Button
            aria-label="Download backup"
            render={<a download href={backupDownloadUrl(app.id, backup.id)} />}
            size="icon-sm"
            variant="ghost"
          >
            <DownloadIcon />
          </Button>
          <Button
            aria-label="Restore backup"
            disabled={busy}
            onClick={confirmRestore}
            size="icon-sm"
            variant="ghost"
          >
            {restore.isPending ? <Loader2Icon className="animate-spin" /> : <ArchiveRestoreIcon />}
          </Button>
          <Button
            aria-label="Delete backup"
            disabled={busy}
            onClick={confirmDelete}
            size="icon-sm"
            variant="ghost"
          >
            {remove.isPending ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
          </Button>
        </div>
      ) : backup.status === "failed" ? (
        <Button
          aria-label="Delete backup"
          disabled={busy}
          onClick={() => remove.mutate()}
          size="icon-sm"
          variant="ghost"
        >
          <Trash2Icon />
        </Button>
      ) : null}
    </div>
  );
}

function BackupStatusBadge({ status }: { status: DatabaseBackup["status"] }) {
  if (status === "completed") return <Badge variant="outline">Done</Badge>;
  if (status === "failed") return <Badge variant="destructive">Failed</Badge>;
  return (
    <Badge variant="secondary">
      <Loader2Icon className="size-3 animate-spin" />
      {status === "running" ? "Running" : "Queued"}
    </Badge>
  );
}

const NO_S3_VALUE = "__none__";

function BackupScheduleCard({
  appId,
  settings,
  s3Connections,
  onSaved,
}: {
  appId: string;
  settings: {
    scheduleEnabled: boolean;
    intervalHours: number;
    retention: number;
    s3ConnectionId: string | null;
  };
  s3Connections: { id: string; name: string; bucket: string }[];
  onSaved: () => void;
}) {
  const [enabled, setEnabled] = useState(settings.scheduleEnabled);
  const [intervalHours, setIntervalHours] = useState(String(settings.intervalHours));
  const [retention, setRetention] = useState(String(settings.retention));
  const [s3ConnectionId, setS3ConnectionId] = useState(settings.s3ConnectionId ?? NO_S3_VALUE);

  useEffect(() => {
    setEnabled(settings.scheduleEnabled);
    setIntervalHours(String(settings.intervalHours));
    setRetention(String(settings.retention));
    setS3ConnectionId(settings.s3ConnectionId ?? NO_S3_VALUE);
  }, [
    settings.scheduleEnabled,
    settings.intervalHours,
    settings.retention,
    settings.s3ConnectionId,
  ]);

  const save = useMutation({
    mutationFn: () =>
      updateBackupSettings(appId, {
        scheduleEnabled: enabled,
        intervalHours: Number(intervalHours),
        retention: Number(retention),
        s3ConnectionId: s3ConnectionId === NO_S3_VALUE ? null : s3ConnectionId,
      }),
    onSuccess: () => {
      toast.success("Backup settings saved");
      onSaved();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  const dirty =
    enabled !== settings.scheduleEnabled ||
    Number(intervalHours) !== settings.intervalHours ||
    Number(retention) !== settings.retention ||
    (s3ConnectionId === NO_S3_VALUE ? null : s3ConnectionId) !== settings.s3ConnectionId;

  return (
    <Card className="p-6">
      <div className="flex items-center justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Schedule</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Automatic backups on a fixed interval, pruned to the retention count.
          </p>
        </div>
        <Switch checked={enabled} onCheckedChange={setEnabled} />
      </div>
      <div className="mt-4 grid grid-cols-2 gap-3">
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="backup-interval">Interval (hours)</Label>
          <Input
            disabled={!enabled}
            id="backup-interval"
            inputMode="numeric"
            max={168}
            min={1}
            onChange={(event) => setIntervalHours(event.target.value)}
            type="number"
            value={intervalHours}
          />
        </div>
        <div className="flex flex-col gap-1.5">
          <Label htmlFor="backup-retention">Keep last</Label>
          <Input
            disabled={!enabled}
            id="backup-retention"
            inputMode="numeric"
            max={50}
            min={1}
            onChange={(event) => setRetention(event.target.value)}
            type="number"
            value={retention}
          />
        </div>
      </div>
      <div className="mt-4 flex flex-col gap-1.5">
        <Label>Upload to S3</Label>
        <Select
          onValueChange={(value) => setS3ConnectionId((value as string) ?? NO_S3_VALUE)}
          value={s3ConnectionId}
        >
          <SelectTrigger className="w-full">
            <SelectValue placeholder="Keep on server only">
              {(value: string) => {
                if (value === NO_S3_VALUE) return "Keep on server only";
                const connection = s3Connections.find((item) => item.id === value);
                return connection
                  ? `${connection.name} (${connection.bucket})`
                  : "Unknown connection";
              }}
            </SelectValue>
          </SelectTrigger>
          <SelectPopup>
            <SelectItem value={NO_S3_VALUE}>Keep on server only</SelectItem>
            {s3Connections.map((connection) => (
              <SelectItem key={connection.id} value={connection.id}>
                {connection.name} ({connection.bucket})
              </SelectItem>
            ))}
          </SelectPopup>
        </Select>
        {s3Connections.length === 0 ? (
          <p className="text-muted-foreground text-xs">
            Add a connection on the S3 storage page to upload backups off this server.
          </p>
        ) : null}
      </div>
      <div className="mt-4 flex justify-end">
        <Button disabled={!dirty || save.isPending} onClick={() => save.mutate()} size="sm">
          {save.isPending ? <Loader2Icon className="animate-spin" /> : null}
          Save settings
        </Button>
      </div>
    </Card>
  );
}
