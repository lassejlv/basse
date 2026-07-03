import { TrashIcon } from "lucide-react";
import type { ReactNode } from "react";
import { Button } from "@/components/ui/button";
import { cn } from "@/lib/utils";

/* Small shared primitives for dashboard pages: the mono section label,
   bordered row lists, empty/error notes, and the row delete button. */

export function SectionLabel({
  as: Tag = "p",
  className,
  children,
}: {
  as?: "p" | "h2" | "h3" | "dt";
  className?: string;
  children: ReactNode;
}) {
  return (
    <Tag
      className={cn(
        "font-mono text-[0.7rem] text-muted-foreground uppercase tracking-[0.14em]",
        className,
      )}
    >
      {children}
    </Tag>
  );
}

export function RowList({ children }: { children: ReactNode }) {
  return <ul className="divide-y rounded-lg border">{children}</ul>;
}

export function Row({ action, children }: { action?: ReactNode; children: ReactNode }) {
  return (
    <li className="flex items-center justify-between gap-3 px-3 py-2">
      <div className="min-w-0">{children}</div>
      {action}
    </li>
  );
}

export function RowDeleteButton({
  label,
  loading,
  onClick,
  confirmMessage,
}: {
  label: string;
  loading?: boolean;
  onClick: () => void;
  confirmMessage?: string;
}) {
  return (
    <Button
      aria-label={label}
      loading={loading}
      onClick={() => {
        if (!confirmMessage || window.confirm(confirmMessage)) onClick();
      }}
      size="icon-sm"
      variant="ghost"
    >
      <TrashIcon />
    </Button>
  );
}

export function EmptyNote({ className, children }: { className?: string; children: ReactNode }) {
  return (
    <p
      className={cn(
        "rounded-lg border border-dashed px-3 py-4 text-center text-muted-foreground text-sm",
        className,
      )}
    >
      {children}
    </p>
  );
}

export function ErrorText({ className, children }: { className?: string; children: ReactNode }) {
  return <p className={cn("text-destructive-foreground text-sm", className)}>{children}</p>;
}
