import { useQuery } from "@tanstack/react-query";
import { CheckIcon, CopyIcon } from "lucide-react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Label } from "@/components/ui/label";
import type { App } from "@/lib/apps";
import { getDatabaseConnectionInfo } from "@/lib/apps";
import { useClipboard } from "./shared";

export function DatabaseConnectionCard({ app }: { app: App }) {
  const { copiedId, copy } = useClipboard();
  const connection = useQuery({
    queryKey: ["database-connection", app.id],
    queryFn: () => getDatabaseConnectionInfo(app.id),
    enabled: app.appKind === "database",
  });

  const internalUri = connection.data?.internalUri ?? "";
  const publicUri = connection.data?.publicUri ?? "";

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Connection</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Use the internal URI from other Basse apps on the same server.
      </p>
      <div className="mt-4 flex flex-col gap-4">
        <ConnectionValue
          copied={copiedId === "internal"}
          label="Internal URI"
          loading={connection.isPending}
          onCopy={() => copy("internal", internalUri)}
          value={internalUri}
        />
        {app.database?.publicEnabled ? (
          <ConnectionValue
            copied={copiedId === "public"}
            label="Public URI"
            loading={connection.isPending}
            onCopy={() => copy("public", publicUri)}
            value={publicUri || "Redeploy after enabling public access."}
          />
        ) : null}
      </div>
      {connection.isError ? (
        <p className="mt-3 text-destructive-foreground text-sm">{connection.error.message}</p>
      ) : null}
    </Card>
  );
}

export function ConnectionValue({
  copied,
  label,
  loading,
  onCopy,
  value,
}: {
  copied: boolean;
  label: string;
  loading: boolean;
  onCopy: () => void;
  value: string;
}) {
  return (
    <div className="space-y-2">
      <Label>{label}</Label>
      <div className="flex min-w-0 items-center gap-2">
        <code className="min-w-0 flex-1 truncate rounded-md border bg-muted/40 px-3 py-2 font-mono text-xs">
          {loading ? "Loading..." : value}
        </code>
        <Button disabled={!value || loading} onClick={onCopy} size="icon" variant="outline">
          {copied ? <CheckIcon /> : <CopyIcon />}
        </Button>
      </div>
    </div>
  );
}
