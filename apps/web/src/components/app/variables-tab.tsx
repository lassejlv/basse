import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { CheckIcon, CopyIcon, EyeIcon, EyeOffIcon, PencilIcon, PlusIcon } from "lucide-react";
import { useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Card } from "@/components/ui/card";
import { Textarea } from "@/components/ui/textarea";
import type { StagedChange } from "@/lib/changes";
import { getEnvDraft, stageEnvVars } from "@/lib/changes";
import { parseDotenv, serializeDotenv } from "@/lib/dotenv";
import { listEnvReferences, listEnvVars, revealEnvVars } from "@/lib/env-vars";
import { toast } from "@/lib/toast";
import { useClipboard } from "./shared";

export function EnvVarsCard({
  appId,
  stagedChanges,
}: {
  appId: string;
  stagedChanges: StagedChange[];
}) {
  const queryClient = useQueryClient();
  const maskedKey = ["env-vars", appId];
  const revealKey = ["env-vars-reveal", appId];
  const referencesKey = ["env-references", appId];
  const stagedEnvCount = stagedChanges.filter((change) => change.resource === "env_var").length;

  const [editing, setEditing] = useState(false);
  const [revealed, setRevealed] = useState(false);
  const [preparing, setPreparing] = useState(false);
  const [draft, setDraft] = useState("");
  const [referenceQuery, setReferenceQuery] = useState("");
  const [error, setError] = useState<string | null>(null);
  const textareaRef = useRef<HTMLTextAreaElement | null>(null);
  const { copiedId, copy } = useClipboard();

  const vars = useQuery({ queryKey: maskedKey, queryFn: () => listEnvVars(appId) });
  const references = useQuery({
    queryKey: referencesKey,
    queryFn: () => listEnvReferences(appId),
    enabled: editing,
  });
  const reveal = useQuery({
    queryKey: revealKey,
    queryFn: () => revealEnvVars(appId),
    enabled: revealed && !editing,
  });

  const list = vars.data ?? [];
  const revealedMap = new Map((reveal.data ?? []).map((v) => [v.key, v.value]));
  const referenceSuggestions = (references.data ?? [])
    .filter((item) => {
      if (!referenceQuery) return true;
      const query = referenceQuery.toLowerCase();
      return item.label.toLowerCase().includes(query) || item.key.toLowerCase().includes(query);
    })
    .slice(0, 8);

  function ensureRevealed() {
    return queryClient.fetchQuery({ queryKey: revealKey, queryFn: () => revealEnvVars(appId) });
  }

  function updateReferenceQuery(value: string, cursor: number) {
    const beforeCursor = value.slice(0, cursor);
    const match = beforeCursor.match(/\{\{\s*(?:shared|env)?\.?([A-Za-z0-9_]*)$/);
    setReferenceQuery(match ? (match[1] ?? "").toLowerCase() : "");
  }

  function insertReference(insertText: string) {
    const textarea = textareaRef.current;
    if (!textarea) {
      setDraft((value) => `${value}${insertText}`);
      return;
    }

    const start = textarea.selectionStart;
    const end = textarea.selectionEnd;
    const beforeCursor = draft.slice(0, start);
    const openToken = beforeCursor.match(/\{\{\s*(?:shared|env)?\.?[A-Za-z0-9_]*$/);
    const replaceFrom = openToken ? start - openToken[0].length : start;
    const next = `${draft.slice(0, replaceFrom)}${insertText}${draft.slice(end)}`;
    const cursor = replaceFrom + insertText.length;
    setDraft(next);
    setReferenceQuery("");
    requestAnimationFrame(() => {
      textarea.focus();
      textarea.setSelectionRange(cursor, cursor);
    });
  }

  const save = useMutation({
    mutationFn: () => stageEnvVars(appId, { vars: parseDotenv(draft) }),
    onSuccess: (data) => {
      setError(null);
      setEditing(false);
      toast.success("Variables staged");
      queryClient.setQueryData(["changes", appId], data);
      void queryClient.invalidateQueries({ queryKey: ["project-changes"] });
    },
    onError: (e: Error) => setError(e.message),
  });

  async function startEdit() {
    setError(null);
    setPreparing(true);
    try {
      // Seed from the draft (live env overlaid with staged edits) so the user
      // keeps building on what is already staged.
      setDraft(serializeDotenv(await getEnvDraft(appId)));
      setEditing(true);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Could not load variables.");
    } finally {
      setPreparing(false);
    }
  }

  async function copyAll() {
    try {
      copy("__all__", serializeDotenv(await ensureRevealed()));
    } catch {
      // Reveal failed — nothing to copy.
    }
  }

  async function copyRow(key: string) {
    try {
      const pairs = await ensureRevealed();
      copy(key, pairs.find((pair) => pair.key === key)?.value ?? "");
    } catch {
      // Reveal failed — nothing to copy.
    }
  }

  return (
    <Card className="p-6">
      <div className="flex flex-wrap items-start justify-between gap-3">
        <div>
          <h2 className="font-semibold text-lg">Environment variables</h2>
          <p className="mt-1 text-muted-foreground text-sm">
            Runtime variables, encrypted at rest. Edits are staged until you deploy.
          </p>
          {stagedEnvCount > 0 ? (
            <p className="mt-1 text-primary text-sm">
              {stagedEnvCount} variable change{stagedEnvCount === 1 ? "" : "s"} staged — review in
              the bar below.
            </p>
          ) : null}
        </div>
        {!editing && list.length > 0 ? (
          <div className="flex items-center gap-2">
            <Button onClick={() => setRevealed((value) => !value)} size="sm" variant="outline">
              {revealed ? <EyeOffIcon /> : <EyeIcon />}
              {revealed ? "Hide" : "Reveal"}
            </Button>
            <Button onClick={copyAll} size="sm" variant="outline">
              {copiedId === "__all__" ? <CheckIcon /> : <CopyIcon />}
              .env
            </Button>
            <Button loading={preparing} onClick={startEdit} size="sm">
              <PencilIcon />
              Edit
            </Button>
          </div>
        ) : null}
      </div>

      {editing ? (
        <form
          className="mt-5 space-y-3"
          onSubmit={(event) => {
            event.preventDefault();
            save.mutate();
          }}
        >
          <Textarea
            autoFocus
            className="min-h-56 font-mono text-xs leading-relaxed"
            onChange={(event) => {
              setDraft(event.currentTarget.value);
              updateReferenceQuery(event.currentTarget.value, event.currentTarget.selectionStart);
            }}
            onKeyUp={(event) =>
              updateReferenceQuery(event.currentTarget.value, event.currentTarget.selectionStart)
            }
            ref={textareaRef}
            spellCheck={false}
            value={draft}
          />
          {referenceSuggestions.length > 0 ? (
            <div className="rounded-md border bg-muted/20 p-3">
              <div className="flex flex-wrap gap-2">
                {referenceSuggestions.map((item) => (
                  <Button
                    key={item.insertText}
                    onClick={() => insertReference(item.insertText)}
                    size="sm"
                    type="button"
                    variant="outline"
                  >
                    <span className="font-mono">{item.label}</span>
                    <span className="text-muted-foreground">{item.valueHint}</span>
                  </Button>
                ))}
              </div>
            </div>
          ) : null}
          <p className="text-muted-foreground text-xs">
            One <code className="font-mono">KEY=value</code> per line. Quote values with spaces, use{" "}
            <code className="font-mono">\n</code> for newlines, <code className="font-mono">#</code>{" "}
            for comments. Reference shared values with{" "}
            <code className="font-mono">{"{{shared.KEY}}"}</code> or{" "}
            <code className="font-mono">{"{{env.KEY}}"}</code>.
          </p>
          {error ? <p className="text-destructive-foreground text-sm">{error}</p> : null}
          <div className="flex gap-2">
            <Button loading={save.isPending} type="submit">
              Stage variables
            </Button>
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
          </div>
        </form>
      ) : vars.isPending ? (
        <p className="mt-5 text-muted-foreground text-sm">Loading…</p>
      ) : list.length === 0 ? (
        <div className="mt-5 flex flex-col items-start gap-3 rounded-md border border-dashed p-5">
          <p className="text-muted-foreground text-sm">No variables set.</p>
          <Button loading={preparing} onClick={startEdit} size="sm" variant="outline">
            <PlusIcon />
            Add variables
          </Button>
        </div>
      ) : (
        <ul className="mt-5 divide-y rounded-md border">
          {list.map((v) => {
            const display = revealed
              ? reveal.isPending
                ? "…"
                : (revealedMap.get(v.key) ?? "")
              : v.valueHint;
            return (
              <li key={v.key} className="flex items-center gap-3 px-3 py-2 font-mono text-sm">
                <span className="min-w-0 max-w-[45%] truncate font-medium text-foreground">
                  {v.key}
                </span>
                <span
                  className="min-w-0 flex-1 truncate text-right text-muted-foreground"
                  title={revealed ? revealedMap.get(v.key) : undefined}
                >
                  {display}
                </span>
                <Button
                  aria-label={`Copy ${v.key}`}
                  className="shrink-0"
                  onClick={() => copyRow(v.key)}
                  size="icon-xs"
                  variant="ghost"
                >
                  {copiedId === v.key ? <CheckIcon /> : <CopyIcon />}
                </Button>
              </li>
            );
          })}
        </ul>
      )}
    </Card>
  );
}
