import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { GitBranchIcon, PlusIcon, TrashIcon } from "lucide-react";
import { useState } from "react";
import type { NeonBranch } from "@basse/shared";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Input } from "@/components/ui/input";
import type { App } from "@/lib/apps";
import {
  createNeonBranch,
  deleteNeonBranch,
  getNeonBranchConnection,
  listNeonBranches,
} from "@/lib/neon";
import { toast, toMessage } from "@/lib/toast";
import { cn } from "@/lib/utils";
import { ConnectionValue } from "./connection-tab";
import { useClipboard } from "./shared";

/** The Neon database experience: branches (create, delete, switch between)
 * and the selected branch's pooled + direct connection strings. */
export function NeonDatabaseTab({ app }: { app: App }) {
  const queryClient = useQueryClient();
  const branchesKey = ["neon-branches", app.id];
  const [selectedBranchId, setSelectedBranchId] = useState<string | null>(null);
  const [newBranchName, setNewBranchName] = useState("");

  const branches = useQuery({
    queryKey: branchesKey,
    queryFn: () => listNeonBranches(app.id),
  });

  const branchList = branches.data ?? [];
  const selectedBranch =
    branchList.find((branch) => branch.id === selectedBranchId) ??
    branchList.find((branch) => branch.isDefault) ??
    branchList[0];

  const create = useMutation({
    mutationFn: () => createNeonBranch(app.id, { name: newBranchName.trim() }),
    onSuccess: async (branch) => {
      setNewBranchName("");
      setSelectedBranchId(branch.id);
      toast.success(`Branch ${branch.name} created`);
      await queryClient.invalidateQueries({ queryKey: branchesKey });
    },
    onError: (error: Error) =>
      toast.error("Couldn't create branch", { description: toMessage(error) }),
  });

  const remove = useMutation({
    mutationFn: (branchId: string) => deleteNeonBranch(app.id, branchId),
    onSuccess: async (_, branchId) => {
      if (selectedBranchId === branchId) setSelectedBranchId(null);
      toast.success("Branch deleted");
      await queryClient.invalidateQueries({ queryKey: branchesKey });
    },
    onError: (error: Error) =>
      toast.error("Couldn't delete branch", { description: toMessage(error) }),
  });

  function confirmDelete(branch: NeonBranch) {
    if (!window.confirm(`Delete branch ${branch.name}? Its data is removed on Neon.`)) return;
    remove.mutate(branch.id);
  }

  return (
    <div className="flex flex-col gap-6">
      <Card className="p-6">
        <div className="flex items-center gap-2">
          <GitBranchIcon className="size-5 text-muted-foreground" />
          <h2 className="font-semibold text-lg">Branches</h2>
        </div>
        <p className="mt-1 text-muted-foreground text-sm">
          Each branch is an isolated copy-on-write Postgres with its own connection strings.
        </p>

        <form
          className="mt-4 flex gap-2"
          onSubmit={(event) => {
            event.preventDefault();
            if (!newBranchName.trim()) return;
            create.mutate();
          }}
        >
          <Input
            aria-label="New branch name"
            onChange={(event) => setNewBranchName(event.currentTarget.value)}
            placeholder="preview/my-feature"
            value={newBranchName}
          />
          <Button disabled={!newBranchName.trim()} loading={create.isPending} type="submit">
            <PlusIcon />
            Create branch
          </Button>
        </form>

        <div className="mt-4 flex flex-col gap-2">
          {branches.isPending ? (
            <p className="text-muted-foreground text-sm">Loading branches…</p>
          ) : branches.isError ? (
            <p className="text-destructive-foreground text-sm">{toMessage(branches.error)}</p>
          ) : branchList.length === 0 ? (
            <p className="text-muted-foreground text-sm">No branches yet.</p>
          ) : (
            branchList.map((branch) => (
              <div
                className={cn(
                  "flex items-center gap-3 rounded-md border px-3 py-2",
                  selectedBranch?.id === branch.id && "border-primary/50 bg-accent/40",
                )}
                key={branch.id}
              >
                <button
                  className="flex min-w-0 flex-1 items-center gap-2 text-left"
                  onClick={() => setSelectedBranchId(branch.id)}
                  type="button"
                >
                  <GitBranchIcon className="size-3.5 shrink-0 text-muted-foreground" />
                  <span className="truncate font-mono text-sm">{branch.name}</span>
                  {branch.isDefault ? (
                    <Badge size="sm" variant="secondary">
                      default
                    </Badge>
                  ) : null}
                </button>
                <span className="shrink-0 text-muted-foreground text-xs">
                  {new Date(branch.createdAt).toLocaleDateString()}
                </span>
                {!branch.isDefault ? (
                  <Button
                    aria-label={`Delete branch ${branch.name}`}
                    loading={remove.isPending && remove.variables === branch.id}
                    onClick={() => confirmDelete(branch)}
                    size="icon-sm"
                    variant="ghost"
                  >
                    <TrashIcon />
                  </Button>
                ) : null}
              </div>
            ))
          )}
        </div>
      </Card>

      {selectedBranch ? <NeonBranchConnectionCard appId={app.id} branch={selectedBranch} /> : null}
    </div>
  );
}

function NeonBranchConnectionCard({ appId, branch }: { appId: string; branch: NeonBranch }) {
  const { copiedId, copy } = useClipboard();
  const connection = useQuery({
    queryKey: ["neon-branch-connection", appId, branch.id],
    queryFn: () => getNeonBranchConnection(appId, branch.id),
  });

  const pooledUri = connection.data?.pooledUri ?? "";
  const directUri = connection.data?.directUri ?? "";

  return (
    <Card className="p-6">
      <h2 className="font-semibold text-lg">Connect</h2>
      <p className="mt-1 text-muted-foreground text-sm">
        Connection strings for <span className="font-mono">{branch.name}</span>. Use the pooled URI
        from apps; the direct URI for migrations and tools that need session semantics.
      </p>
      <div className="mt-4 flex flex-col gap-4">
        <ConnectionValue
          copied={copiedId === "pooled"}
          label="Pooled connection string"
          loading={connection.isPending}
          onCopy={() => copy("pooled", pooledUri)}
          value={pooledUri}
        />
        <ConnectionValue
          copied={copiedId === "direct"}
          label="Direct connection string"
          loading={connection.isPending}
          onCopy={() => copy("direct", directUri)}
          value={directUri}
        />
      </div>
      {connection.isError ? (
        <p className="mt-3 text-destructive-foreground text-sm">{toMessage(connection.error)}</p>
      ) : null}
    </Card>
  );
}
