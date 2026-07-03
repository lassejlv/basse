import { Link } from "@tanstack/react-router";
import { ArrowUpIcon, CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ThemeToggle } from "@/components/theme-toggle";
import { cn } from "@/lib/utils";

const INSTALL_DISPLAY = "curl -fsSL basse.sh/install | bash";
const INSTALL_COMMAND = "curl -fsSL https://basse.sh/install | bash";

const navLinkClass = "px-1 py-1.5 text-sm font-medium text-muted-foreground hover:text-foreground";

export function SiteHeader() {
  return (
    <header className="sticky top-0 z-50 border-b border-transparent bg-background/75 backdrop-blur-md backdrop-saturate-[1.8]">
      <div className="mx-auto flex h-16 max-w-[1120px] items-center gap-4 px-7">
        <Link to="/" className="flex items-center gap-[9px]">
          <span className="inline-flex size-6 items-center justify-center rounded-[7px] bg-primary text-primary-foreground">
            <ArrowUpIcon className="size-3.5" strokeWidth={2.6} />
          </span>
          <span className="text-base font-semibold tracking-[-0.01em]">basse</span>
        </Link>
        <div className="flex-1" />
        <Link
          to="/pricing"
          className={navLinkClass}
          activeProps={{ className: "px-1 py-1.5 text-sm font-medium text-foreground" }}
        >
          Pricing
        </Link>
        <a href="#" className={navLinkClass}>
          Docs
        </a>
        <a href="#" className={navLinkClass}>
          GitHub
        </a>
        <ThemeToggle />
        <Link
          to="/dashboard"
          className="inline-flex h-8 items-center rounded-[9px] bg-primary px-[13px] text-sm font-medium text-primary-foreground hover:bg-primary/90"
        >
          Deploy
        </Link>
      </div>
    </header>
  );
}

export function SiteFooter() {
  return (
    <footer className="border-t">
      <div className="mx-auto flex max-w-[1120px] flex-wrap items-center justify-between gap-4 px-7 py-6">
        <div className="flex items-center gap-[9px]">
          <span className="inline-flex size-5 items-center justify-center rounded-md bg-primary text-primary-foreground">
            <ArrowUpIcon className="size-[11px]" strokeWidth={2.6} />
          </span>
          <span className="font-mono text-[13px] font-medium text-muted-foreground">basse</span>
        </div>
        <div className="flex gap-5 font-mono text-[12.5px] text-muted-foreground">
          <Link to="/pricing" className="hover:text-foreground">
            Pricing
          </Link>
          <a href="#" className="hover:text-foreground">
            Docs
          </a>
          <a href="#" className="hover:text-foreground">
            GitHub
          </a>
          <span>MIT licensed</span>
        </div>
      </div>
    </footer>
  );
}

/* The canvas dot grid from the dashboard, fading out toward the page
   background. `fadeClassName` positions the fade per page. */
export function DotGrid({ fadeClassName }: { fadeClassName: string }) {
  return (
    <>
      <div
        aria-hidden
        className="absolute inset-0"
        style={{
          backgroundImage: "radial-gradient(var(--color-border) 1px, transparent 1px)",
          backgroundSize: "24px 24px",
        }}
      />
      <div aria-hidden className={cn("absolute inset-0", fadeClassName)} />
    </>
  );
}

export function InstallCommand({ className }: { className?: string }) {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => clearTimeout(timeoutRef.current ?? undefined), []);

  function copy() {
    navigator.clipboard?.writeText(INSTALL_COMMAND).catch(() => {});
    setCopied(true);
    clearTimeout(timeoutRef.current ?? undefined);
    timeoutRef.current = setTimeout(() => setCopied(false), 1600);
  }

  return (
    <div
      className={cn(
        "flex h-[42px] max-w-full items-center gap-3 rounded-[11px] border pr-[5px] pl-[15px]",
        className,
      )}
    >
      <span className="font-mono text-[13.5px] font-medium text-muted-foreground">$</span>
      <span className="min-w-0 flex-1 truncate font-mono text-[13.5px] font-medium text-foreground">
        {INSTALL_DISPLAY}
      </span>
      <button
        type="button"
        onClick={copy}
        aria-label="Copy install command"
        className="inline-flex h-8 shrink-0 items-center gap-1.5 rounded-lg px-2.5 text-[13px] font-medium text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
      >
        {copied ? (
          <CheckIcon className="size-3.5 text-success-foreground" strokeWidth={2.4} />
        ) : (
          <CopyIcon className="size-3.5 opacity-80" />
        )}
        {copied ? "Copied" : "Copy"}
      </button>
    </div>
  );
}
