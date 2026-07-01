import {
  ArrowDownToLineIcon,
  CheckIcon,
  ChevronRightIcon,
  CopyIcon,
  RefreshCwIcon,
  SearchIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";
import { Button } from "@/components/ui/button";
import { Kbd } from "@/components/ui/kbd";
import { cn } from "@/lib/utils";

export type LogLevel = "info" | "warn" | "error";

export type LogLine = {
  id: number;
  /** Display time (already formatted), or null when the line carries none. */
  time: string | null;
  level: LogLevel;
  text: string;
  /** Pretty-printed structured payload for JSON lines — expandable in the UI. */
  detail?: string;
};

// eslint-disable-next-line no-control-regex
const ANSI_PATTERN = /\u001B\[[0-9;]*[A-Za-z]/g;
const ISO_TIME_PATTERN = /^\[?(\d{4}-\d{2}-\d{2})[T ](\d{2}:\d{2}:\d{2})(?:[.,]\d+)?(?:Z|[+-]\d{2}:?\d{2})?\]?\s*/;
const ERROR_PATTERN = /\b(error|err|fatal|panic|exception|failed|failure)\b/i;
const WARN_PATTERN = /\bwarn(ing)?\b/i;

export function detectLogLevel(text: string): LogLevel {
  if (ERROR_PATTERN.test(text)) return "error";
  if (WARN_PATTERN.test(text)) return "warn";
  return "info";
}

function levelFromJson(value: unknown): LogLevel | null {
  if (typeof value !== "string") return null;
  const normalized = value.toLowerCase();
  if (["error", "err", "fatal", "panic", "crit", "critical"].includes(normalized)) return "error";
  if (["warn", "warning"].includes(normalized)) return "warn";
  if (["info", "debug", "trace", "notice"].includes(normalized)) return "info";
  return null;
}

function pad2(value: number): string {
  return String(value).padStart(2, "0");
}

function timeFromJson(obj: Record<string, unknown>): string | null {
  // Unix seconds (Caddy's `ts`) or an ISO string under common keys.
  const numeric = obj.ts ?? obj.timestamp;
  if (typeof numeric === "number" && Number.isFinite(numeric)) {
    const date = new Date(numeric > 1e12 ? numeric : numeric * 1000);
    return `${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`;
  }
  const iso = obj.time ?? obj.timestamp ?? obj["@timestamp"];
  if (typeof iso === "string") {
    const date = new Date(iso);
    if (!Number.isNaN(date.getTime())) {
      return `${pad2(date.getHours())}:${pad2(date.getMinutes())}:${pad2(date.getSeconds())}`;
    }
  }
  return null;
}

function formatDuration(seconds: number): string {
  if (seconds < 0.001) return `${(seconds * 1_000_000).toFixed(0)}µs`;
  if (seconds < 1) return `${(seconds * 1000).toFixed(1)}ms`;
  return `${seconds.toFixed(2)}s`;
}

/** One readable line out of a structured log object: the message first, then
 * the request facts everyone actually scans for (method, path, status, time). */
function summarizeJson(obj: Record<string, unknown>): string {
  const parts: string[] = [];
  const message = obj.msg ?? obj.message ?? obj.event;
  const logger = obj.logger ?? obj.component;
  if (typeof logger === "string" && logger.length > 0) parts.push(`[${logger}]`);
  if (typeof message === "string" && message.length > 0) parts.push(message);

  const request = (obj.request ?? {}) as Record<string, unknown>;
  const method = request.method ?? obj.method;
  const uri = request.uri ?? obj.uri ?? obj.path;
  if (typeof method === "string" && typeof uri === "string") {
    parts.push(`${method} ${uri}`);
  }
  const status = obj.status ?? obj.status_code;
  if (typeof status === "number") parts.push(`→ ${status}`);
  const duration = obj.duration;
  if (typeof duration === "number") parts.push(`in ${formatDuration(duration)}`);

  const error = obj.error ?? obj.err;
  if (typeof error === "string" && error.length > 0) parts.push(`error=${error}`);

  return parts.join(" ");
}

function parseJsonLine(
  text: string,
): { time: string | null; level: LogLevel | null; text: string; detail: string } | null {
  const trimmed = text.trim();
  if (!trimmed.startsWith("{") || !trimmed.endsWith("}")) return null;
  try {
    const obj = JSON.parse(trimmed) as unknown;
    if (typeof obj !== "object" || obj === null || Array.isArray(obj)) return null;
    const record = obj as Record<string, unknown>;
    const summary = summarizeJson(record);
    return {
      time: timeFromJson(record),
      level: levelFromJson(record.level ?? record.severity ?? record.lvl),
      text: summary.length > 0 ? summary : trimmed.slice(0, 200),
      detail: JSON.stringify(record, null, 2),
    };
  } catch {
    return null;
  }
}

/** Parse raw log text into display lines: strips ANSI color codes, lifts a
 * leading ISO timestamp into the time column, classifies severity, and turns
 * valid JSON lines into a summary with an expandable pretty-printed payload. */
export function parseLogs(raw: string): LogLine[] {
  return raw
    .replace(ANSI_PATTERN, "")
    .split("\n")
    .filter((line) => line.trim().length > 0)
    .map((line, index) => {
      const match = line.match(ISO_TIME_PATTERN);
      const text = match ? line.slice(match[0].length) : line;

      const json = parseJsonLine(text);
      if (json) {
        return {
          id: index,
          time: json.time ?? (match ? (match[2] ?? null) : null),
          level: json.level ?? detectLogLevel(json.text),
          text: json.text,
          detail: json.detail,
        };
      }

      return {
        id: index,
        time: match ? (match[2] ?? null) : null,
        level: detectLogLevel(text),
        text: text.length > 0 ? text : line,
      };
    });
}

const railColor: Record<LogLevel, string> = {
  info: "bg-border",
  warn: "bg-warning",
  error: "bg-destructive",
};

const textColor: Record<LogLevel, string> = {
  info: "text-foreground/85",
  warn: "text-warning-foreground",
  error: "text-destructive-foreground",
};

function HighlightedText({ text, query }: { text: string; query: string }) {
  if (!query) return <>{text}</>;
  const lower = text.toLowerCase();
  const needle = query.toLowerCase();
  const parts: ReactNode[] = [];
  let cursor = 0;
  let hit = lower.indexOf(needle);
  while (hit !== -1) {
    if (hit > cursor) parts.push(text.slice(cursor, hit));
    parts.push(
      <mark className="rounded-[3px] bg-primary/30 text-inherit" key={`${hit}-${cursor}`}>
        {text.slice(hit, hit + needle.length)}
      </mark>,
    );
    cursor = hit + needle.length;
    hit = lower.indexOf(needle, cursor);
  }
  if (cursor < text.length) parts.push(text.slice(cursor));
  return <>{parts}</>;
}

/**
 * The shared log surface: search (press / to focus), severity filter,
 * severity rails on each row, tail-follow while new lines stream in,
 * copy and download. Feed it raw `text` or pre-parsed `lines`.
 */
export function LogExplorer({
  text,
  lines,
  meta,
  live = false,
  onRefresh,
  isRefreshing = false,
  downloadName,
  emptyText = "No logs yet.",
  className,
  maxHeight = "24rem",
}: {
  text?: string;
  lines?: LogLine[];
  /** Right-aligned toolbar slot (e.g. a build id or server picker). */
  meta?: ReactNode;
  /** Renders the pulsing live dot — set while the source is polling. */
  live?: boolean;
  onRefresh?: () => void;
  isRefreshing?: boolean;
  /** Enables the download button, used as the file name. */
  downloadName?: string;
  emptyText?: string;
  className?: string;
  maxHeight?: string;
}) {
  const [query, setQuery] = useState("");
  const [levelFilter, setLevelFilter] = useState<LogLevel | null>(null);
  const [copied, setCopied] = useState(false);
  const [expanded, setExpanded] = useState<ReadonlySet<number>>(new Set());
  const searchRef = useRef<HTMLInputElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const followRef = useRef(true);

  const allLines = useMemo(() => lines ?? parseLogs(text ?? ""), [lines, text]);
  const counts = useMemo(() => {
    let warn = 0;
    let error = 0;
    for (const line of allLines) {
      if (line.level === "warn") warn += 1;
      else if (line.level === "error") error += 1;
    }
    return { warn, error };
  }, [allLines]);

  const visible = useMemo(() => {
    const needle = query.trim().toLowerCase();
    return allLines.filter((line) => {
      if (levelFilter && line.level !== levelFilter) return false;
      if (
        needle &&
        !line.text.toLowerCase().includes(needle) &&
        !line.detail?.toLowerCase().includes(needle)
      ) {
        return false;
      }
      return true;
    });
  }, [allLines, levelFilter, query]);

  // Tail-follow: stick to the bottom while the user hasn't scrolled away.
  useEffect(() => {
    const el = scrollRef.current;
    if (el && followRef.current) el.scrollTop = el.scrollHeight;
  }, [visible.length, allLines.length]);

  // "/" focuses search when no other field owns the keyboard.
  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "/" || event.metaKey || event.ctrlKey || event.altKey) return;
      const target = event.target as HTMLElement | null;
      if (target && /^(INPUT|TEXTAREA|SELECT)$/.test(target.tagName)) return;
      if (target?.isContentEditable) return;
      event.preventDefault();
      searchRef.current?.focus();
    }
    window.addEventListener("keydown", onKeyDown);
    return () => window.removeEventListener("keydown", onKeyDown);
  }, []);

  function copyVisible() {
    const payload = visible
      .map((line) => (line.time ? `${line.time}  ${line.text}` : line.text))
      .join("\n");
    void navigator.clipboard.writeText(payload).then(() => {
      setCopied(true);
      setTimeout(() => setCopied(false), 1500);
    });
  }

  function download() {
    const payload = allLines
      .map((line) => (line.time ? `${line.time}  ${line.text}` : line.text))
      .join("\n");
    const blob = new Blob([payload], { type: "text/plain" });
    const url = URL.createObjectURL(blob);
    const anchor = document.createElement("a");
    anchor.href = url;
    anchor.download = downloadName ?? "logs.txt";
    anchor.click();
    URL.revokeObjectURL(url);
  }

  const filtering = Boolean(query.trim()) || levelFilter !== null;

  return (
    <div className={cn("overflow-hidden rounded-lg border bg-background", className)}>
      <div className="flex flex-wrap items-center gap-2 border-b px-2.5 py-2">
        <div className="flex min-w-0 flex-1 items-center gap-2 rounded-md border bg-muted/20 px-2.5 py-1.5 focus-within:ring-1 focus-within:ring-ring">
          <SearchIcon className="size-3.5 shrink-0 text-muted-foreground" />
          <input
            className="min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-muted-foreground"
            onChange={(event) => setQuery(event.currentTarget.value)}
            placeholder="Filter logs"
            ref={searchRef}
            spellCheck={false}
            type="text"
            value={query}
          />
          <Kbd className="hidden sm:inline-flex">/</Kbd>
        </div>
        {counts.warn > 0 ? (
          <button
            className={cn(
              "rounded-md border px-2 py-1 font-mono text-warning-foreground text-xs transition",
              levelFilter === "warn" ? "border-warning/50 bg-warning/10" : "hover:bg-accent/50",
            )}
            onClick={() => setLevelFilter((level) => (level === "warn" ? null : "warn"))}
            type="button"
          >
            {counts.warn} warn
          </button>
        ) : null}
        {counts.error > 0 ? (
          <button
            className={cn(
              "rounded-md border px-2 py-1 font-mono text-destructive-foreground text-xs transition",
              levelFilter === "error"
                ? "border-destructive/50 bg-destructive/10"
                : "hover:bg-accent/50",
            )}
            onClick={() => setLevelFilter((level) => (level === "error" ? null : "error"))}
            type="button"
          >
            {counts.error} errors
          </button>
        ) : null}
        {meta}
        {live ? (
          <span className="inline-flex items-center gap-1.5 px-1 text-muted-foreground text-xs">
            <span className="relative inline-flex size-1.5">
              <span className="absolute inline-flex size-full animate-ping rounded-full bg-success opacity-60 motion-reduce:hidden" />
              <span className="relative inline-flex size-full rounded-full bg-success" />
            </span>
            Live
          </span>
        ) : null}
        {onRefresh ? (
          <Button
            aria-label="Refresh logs"
            disabled={isRefreshing}
            onClick={onRefresh}
            size="icon-sm"
            variant="ghost"
          >
            <RefreshCwIcon className={cn(isRefreshing && "animate-spin")} />
          </Button>
        ) : null}
        <Button
          aria-label="Copy logs"
          disabled={visible.length === 0}
          onClick={copyVisible}
          size="icon-sm"
          variant="ghost"
        >
          {copied ? <CheckIcon /> : <CopyIcon />}
        </Button>
        {downloadName ? (
          <Button
            aria-label="Download logs"
            disabled={allLines.length === 0}
            onClick={download}
            size="icon-sm"
            variant="ghost"
          >
            <ArrowDownToLineIcon />
          </Button>
        ) : null}
      </div>

      <div
        className="overflow-auto overscroll-contain"
        onScroll={(event) => {
          const el = event.currentTarget;
          followRef.current = el.scrollHeight - el.scrollTop - el.clientHeight < 24;
        }}
        ref={scrollRef}
        style={{ maxHeight }}
      >
        {visible.length === 0 ? (
          <p className="px-4 py-10 text-center text-muted-foreground text-sm">
            {filtering && allLines.length > 0 ? "No lines match the filter." : emptyText}
          </p>
        ) : (
          <div className="py-1.5">
            {visible.map((line) => {
              const isExpanded = Boolean(line.detail) && expanded.has(line.id);
              const row = (
                <>
                  <span
                    aria-hidden
                    className={cn(
                      "w-0.5 shrink-0 self-stretch rounded-full",
                      railColor[line.level],
                    )}
                  />
                  {line.time ? (
                    <span className="shrink-0 select-none pt-px text-muted-foreground/60">
                      {line.time}
                    </span>
                  ) : null}
                  <span
                    className={cn("min-w-0 whitespace-pre-wrap break-all", textColor[line.level])}
                  >
                    <HighlightedText query={query.trim()} text={line.text} />
                  </span>
                  {line.detail ? (
                    <ChevronRightIcon
                      aria-hidden
                      className={cn(
                        "ml-auto size-3 shrink-0 self-center text-muted-foreground/50 transition-transform",
                        isExpanded && "rotate-90",
                      )}
                    />
                  ) : null}
                </>
              );

              return line.detail ? (
                <div key={line.id}>
                  <button
                    aria-expanded={isExpanded}
                    className="flex w-full gap-2.5 px-2.5 py-px text-left font-mono text-xs leading-relaxed outline-none hover:bg-accent/30 focus-visible:bg-accent/30"
                    onClick={() =>
                      setExpanded((current) => {
                        const next = new Set(current);
                        if (next.has(line.id)) next.delete(line.id);
                        else next.add(line.id);
                        return next;
                      })
                    }
                    type="button"
                  >
                    {row}
                  </button>
                  {isExpanded ? (
                    <pre className="mx-2.5 my-1 overflow-x-auto rounded-md border bg-muted/20 p-2.5 font-mono text-[11px] leading-relaxed text-muted-foreground">
                      {line.detail}
                    </pre>
                  ) : null}
                </div>
              ) : (
                <div
                  className="flex gap-2.5 px-2.5 py-px font-mono text-xs leading-relaxed hover:bg-accent/30"
                  key={line.id}
                >
                  {row}
                </div>
              );
            })}
          </div>
        )}
      </div>

      <div className="flex items-center justify-between border-t px-3 py-1.5 text-muted-foreground text-xs">
        <span className="font-mono">
          {filtering ? `${visible.length} of ${allLines.length} lines` : `${allLines.length} lines`}
        </span>
        {filtering ? (
          <button
            className="underline-offset-4 transition hover:text-foreground hover:underline"
            onClick={() => {
              setQuery("");
              setLevelFilter(null);
            }}
            type="button"
          >
            Clear filters
          </button>
        ) : null}
      </div>
    </div>
  );
}
