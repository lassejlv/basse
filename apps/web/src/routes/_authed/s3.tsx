import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { createFileRoute } from "@tanstack/react-router";
import { CloudIcon, Loader2Icon, PlugZapIcon, PlusIcon, Trash2Icon } from "lucide-react";
import { useState } from "react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import { Label } from "@/components/ui/label";
import { relativeTime } from "@/lib/format";
import {
  createS3Connection,
  deleteS3Connection,
  listS3Connections,
  type S3Connection,
  testS3Connection,
} from "@/lib/s3";
import { toast, toMessage } from "@/lib/toast";

export const Route = createFileRoute("/_authed/s3")({
  component: S3Route,
});

function S3Route() {
  const queryClient = useQueryClient();
  const [showForm, setShowForm] = useState(false);
  const connections = useQuery({
    queryKey: ["s3-connections"],
    queryFn: listS3Connections,
  });
  const list = connections.data ?? [];

  const invalidate = () => queryClient.invalidateQueries({ queryKey: ["s3-connections"] });

  return (
    <section className="flex flex-1 flex-col gap-6 p-4 md:p-6">
      <div className="flex flex-wrap items-end justify-between gap-4">
        <div>
          <h1 className="font-semibold text-2xl tracking-tight md:text-3xl">S3 storage</h1>
          <p className="mt-1 text-muted-foreground text-sm">
            S3-compatible connections (AWS S3, Cloudflare R2, MinIO, …) used as off-server
            destinations for database backups.
          </p>
        </div>
        <Button onClick={() => setShowForm((value) => !value)} size="sm">
          <PlusIcon />
          Add connection
        </Button>
      </div>

      {showForm ? (
        <NewConnectionCard
          onCreated={async () => {
            setShowForm(false);
            await invalidate();
          }}
        />
      ) : null}

      {connections.isPending ? (
        <p className="text-muted-foreground text-sm">Loading connections…</p>
      ) : connections.isError ? (
        <p className="text-destructive-foreground text-sm">{toMessage(connections.error)}</p>
      ) : list.length === 0 ? (
        <Card className="flex flex-col items-center gap-2 p-10 text-center">
          <CloudIcon className="size-8 text-muted-foreground" />
          <p className="font-medium">No S3 connections yet</p>
          <p className="text-muted-foreground text-sm">
            Add a bucket to upload database backups off your servers.
          </p>
        </Card>
      ) : (
        <div className="grid gap-3 lg:grid-cols-2">
          {list.map((connection) => (
            <ConnectionCard connection={connection} key={connection.id} onChanged={invalidate} />
          ))}
        </div>
      )}
    </section>
  );
}

function ConnectionCard({
  connection,
  onChanged,
}: {
  connection: S3Connection;
  onChanged: () => void;
}) {
  const test = useMutation({
    mutationFn: () => testS3Connection(connection.id),
    onSuccess: (result) => {
      if (result.status === "active") toast.success("Connection works");
      else toast.error(result.statusMessage ?? "Connection failed");
      onChanged();
    },
    onError: (error) => toast.error(toMessage(error)),
  });
  const remove = useMutation({
    mutationFn: () => deleteS3Connection(connection.id),
    onSuccess: () => {
      toast.success("Connection deleted");
      onChanged();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  function confirmDelete() {
    if (
      window.confirm(
        `Delete ${connection.name}? Apps using it for backup uploads will be detached. Objects already in the bucket are kept.`,
      )
    ) {
      remove.mutate();
    }
  }

  return (
    <Card className="p-5">
      <div className="flex items-start justify-between gap-3">
        <div className="min-w-0">
          <div className="flex items-center gap-2">
            <h2 className="truncate font-semibold">{connection.name}</h2>
            {connection.status === "active" ? (
              <Badge variant="outline">Connected</Badge>
            ) : (
              <Badge variant="destructive">Error</Badge>
            )}
          </div>
          <p className="mt-1 truncate font-mono text-muted-foreground text-xs">
            {connection.bucket}
            {connection.region ? ` · ${connection.region}` : ""}
          </p>
          {connection.endpoint ? (
            <p className="truncate font-mono text-muted-foreground text-xs">
              {connection.endpoint}
            </p>
          ) : null}
        </div>
        <div className="flex shrink-0 items-center gap-1">
          <Button
            aria-label="Test connection"
            disabled={test.isPending}
            onClick={() => test.mutate()}
            size="icon-sm"
            variant="ghost"
          >
            {test.isPending ? <Loader2Icon className="animate-spin" /> : <PlugZapIcon />}
          </Button>
          <Button
            aria-label="Delete connection"
            disabled={remove.isPending}
            onClick={confirmDelete}
            size="icon-sm"
            variant="ghost"
          >
            {remove.isPending ? <Loader2Icon className="animate-spin" /> : <Trash2Icon />}
          </Button>
        </div>
      </div>
      <div className="mt-3 flex flex-wrap items-center gap-x-3 gap-y-1 text-muted-foreground text-xs">
        <span className="font-mono">
          {connection.accessKeyId.slice(0, 6)}… / {connection.secretHint ?? "…"}
        </span>
        <span>Added {relativeTime(connection.createdAt)}</span>
      </div>
      {connection.status === "error" && connection.statusMessage ? (
        <p className="mt-2 text-destructive-foreground text-xs">{connection.statusMessage}</p>
      ) : null}
    </Card>
  );
}

function NewConnectionCard({ onCreated }: { onCreated: () => Promise<void> }) {
  const [name, setName] = useState("");
  const [bucket, setBucket] = useState("");
  const [endpoint, setEndpoint] = useState("");
  const [region, setRegion] = useState("");
  const [accessKeyId, setAccessKeyId] = useState("");
  const [secretAccessKey, setSecretAccessKey] = useState("");

  const create = useMutation({
    mutationFn: () =>
      createS3Connection({
        name: name.trim(),
        bucket: bucket.trim(),
        endpoint: endpoint.trim() || undefined,
        region: region.trim() || undefined,
        accessKeyId: accessKeyId.trim(),
        secretAccessKey,
      }),
    onSuccess: async (created) => {
      if (created.status === "active") toast.success("S3 connection added");
      else toast.error(created.statusMessage ?? "Saved, but the connection test failed");
      await onCreated();
    },
    onError: (error) => toast.error(toMessage(error)),
  });

  const valid = name.trim() && bucket.trim() && accessKeyId.trim() && secretAccessKey;

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">New connection</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Credentials are encrypted at rest. Leave the endpoint empty for AWS S3; set it for R2/MinIO
        (e.g. https://&lt;account&gt;.r2.cloudflarestorage.com).
      </p>
      <div className="mt-4 grid gap-3 sm:grid-cols-2">
        <FieldInput
          id="s3-name"
          label="Name"
          onChange={setName}
          placeholder="Production backups"
          value={name}
        />
        <FieldInput
          id="s3-bucket"
          label="Bucket"
          onChange={setBucket}
          placeholder="my-backups"
          value={bucket}
        />
        <FieldInput
          id="s3-endpoint"
          label="Endpoint (optional)"
          onChange={setEndpoint}
          placeholder="https://s3.example.com"
          value={endpoint}
        />
        <FieldInput
          id="s3-region"
          label="Region (optional)"
          onChange={setRegion}
          placeholder="us-east-1"
          value={region}
        />
        <FieldInput
          id="s3-access-key"
          label="Access key id"
          onChange={setAccessKeyId}
          placeholder="AKIA…"
          value={accessKeyId}
        />
        <FieldInput
          id="s3-secret-key"
          label="Secret access key"
          onChange={setSecretAccessKey}
          type="password"
          value={secretAccessKey}
        />
      </div>
      <div className="mt-4 flex justify-end">
        <Button disabled={!valid || create.isPending} onClick={() => create.mutate()} size="sm">
          {create.isPending ? <Loader2Icon className="animate-spin" /> : null}
          Save and test
        </Button>
      </div>
    </Card>
  );
}

function FieldInput({
  id,
  label,
  onChange,
  placeholder,
  type = "text",
  value,
}: {
  id: string;
  label: string;
  onChange: (value: string) => void;
  placeholder?: string;
  type?: string;
  value: string;
}) {
  return (
    <div className="flex flex-col gap-1.5">
      <Label htmlFor={id}>{label}</Label>
      <Input
        id={id}
        onChange={(event) => onChange(event.target.value)}
        placeholder={placeholder}
        type={type}
        value={value}
      />
    </div>
  );
}
