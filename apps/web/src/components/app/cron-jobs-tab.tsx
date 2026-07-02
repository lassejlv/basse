import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { Loader2Icon, PlayIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import type { App } from "@/lib/apps";
import {
  createCronJob,
  deleteCronJob,
  listCronJobs,
  runCronJob,
  updateCronJob,
} from "@/lib/cron-jobs";
import { relativeTime } from "@/lib/format";
import { toast, toMessage } from "@/lib/toast";

export function CronJobsTab({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const queryKey = ["cron-jobs", app.id];
  const jobs = useQuery({ queryKey, queryFn: () => listCronJobs(app.id) });
  const invalidate = () => queryClient.invalidateQueries({ queryKey });

  const [name, setName] = useState("");
  const [schedule, setSchedule] = useState("*/5 * * * *");
  const [command, setCommand] = useState("");

  const create = useMutation({
    mutationFn: () => createCronJob(app.id, { name, schedule, command }),
    onSuccess: async () => {
      setName("");
      setCommand("");
      toast.success("Cron job created");
      await invalidate();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  return (
    <div className="flex flex-col gap-4">
      <Card className="p-6">
        <h2 className="font-semibold text-lg">Cron jobs</h2>
        <p className="mt-1 text-muted-foreground text-sm">
          Run shell commands inside this app container on a cron schedule.
        </p>
        <form
          className="mt-4 space-y-3"
          onSubmit={(event) => {
            event.preventDefault();
            create.mutate();
          }}
        >
          <div className="grid gap-3 sm:grid-cols-[1fr_150px]">
            <div className="space-y-2">
              <Label htmlFor="cron-name">Name</Label>
              <Input
                id="cron-name"
                onChange={(event) => setName(event.currentTarget.value)}
                placeholder="Clear cache"
                value={name}
              />
            </div>
            <div className="space-y-2">
              <Label htmlFor="cron-schedule">Schedule</Label>
              <Input
                id="cron-schedule"
                onChange={(event) => setSchedule(event.currentTarget.value)}
                value={schedule}
              />
            </div>
          </div>
          <div className="space-y-2">
            <Label htmlFor="cron-command">Command</Label>
            <Input
              id="cron-command"
              onChange={(event) => setCommand(event.currentTarget.value)}
              placeholder="bun run cleanup"
              value={command}
            />
          </div>
          <Button loading={create.isPending} type="submit">
            <PlusIcon />
            Add job
          </Button>
        </form>
      </Card>

      <Card className="p-6">
        <div className="flex flex-col gap-2">
          {jobs.isPending ? (
            <p className="text-muted-foreground text-sm">Loading cron jobs…</p>
          ) : jobs.isError ? (
            <p className="text-destructive-foreground text-sm">{toMessage(jobs.error)}</p>
          ) : (jobs.data ?? []).length === 0 ? (
            <p className="text-muted-foreground text-sm">No cron jobs yet.</p>
          ) : (
            jobs.data?.map((job) => (
              <CronJobRow app={app} job={job} key={job.id} onChanged={invalidate} />
            ))
          )}
        </div>
      </Card>
    </div>
  );
}

function CronJobRow({
  app,
  job,
  onChanged,
}: {
  app: App;
  job: Awaited<ReturnType<typeof listCronJobs>>[number];
  onChanged: () => void;
}) {
  const toggle = useMutation({
    mutationFn: () => updateCronJob(app.id, job.id, { enabled: !job.enabled }),
    onSuccess: onChanged,
    onError: (error) => toast.error(toMessage(error)),
  });
  const run = useMutation({
    mutationFn: () => runCronJob(app.id, job.id),
    onSuccess: () => {
      toast.success("Cron job queued");
      onChanged();
    },
    onError: (error) => toast.error(toMessage(error)),
  });
  const remove = useMutation({
    mutationFn: () => deleteCronJob(app.id, job.id),
    onSuccess: onChanged,
    onError: (error) => toast.error(toMessage(error)),
  });

  return (
    <div className="flex items-center gap-3 rounded-lg border px-3 py-2">
      <Badge variant={job.enabled ? "success" : "secondary"}>
        {job.enabled ? "Enabled" : "Paused"}
      </Badge>
      <div className="min-w-0 flex-1">
        <p className="truncate font-medium text-sm">{job.name}</p>
        <p className="font-mono text-muted-foreground text-xs">{job.schedule}</p>
        <p className="truncate text-muted-foreground text-xs">
          {job.lastRunAt ? `Last run ${relativeTime(job.lastRunAt)}` : "Never run"}
          {job.lastStatus ? ` · ${job.lastStatus}` : ""}
          {job.lastOutput ? ` · ${job.lastOutput}` : ""}
        </p>
      </div>
      <Button
        disabled={toggle.isPending}
        onClick={() => toggle.mutate()}
        size="sm"
        variant="outline"
      >
        {job.enabled ? "Pause" : "Enable"}
      </Button>
      <Button
        aria-label="Run cron job"
        disabled={run.isPending}
        onClick={() => run.mutate()}
        size="icon-sm"
        variant="ghost"
      >
        {run.isPending ? <Loader2Icon className="animate-spin" /> : <PlayIcon />}
      </Button>
      <Button
        aria-label="Delete cron job"
        disabled={remove.isPending}
        onClick={() => remove.mutate()}
        size="icon-sm"
        variant="ghost"
      >
        {remove.isPending ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
      </Button>
    </div>
  );
}
