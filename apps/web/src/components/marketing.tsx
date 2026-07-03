import { Link } from "@tanstack/react-router";
import { CheckIcon, CopyIcon } from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ThemeToggle } from "@/components/theme-toggle";
import { cn } from "@/lib/utils";

const INSTALL_DISPLAY = "curl -fsSL basse.sh/install | bash";
const INSTALL_COMMAND = "curl -fsSL https://basse.sh/install | bash";

export const GITHUB_URL = "https://github.com/lassejlv/basse";

const navLinkClass = "px-1 py-1.5 text-sm font-medium text-muted-foreground hover:text-foreground";

export function Logo({ className }: { className?: string }) {
  return (
    <svg viewBox="0 0 24 24" aria-hidden className={cn("size-6", className)}>
      <rect width="24" height="24" rx="7" className="fill-primary" />
      <g
        transform="translate(5 5) scale(0.5833)"
        fill="none"
        strokeWidth={2.6}
        strokeLinecap="round"
        strokeLinejoin="round"
        className="stroke-primary-foreground"
      >
        <path d="m5 12 7-7 7 7" />
        <path d="M12 19V5" />
      </g>
    </svg>
  );
}

export function SiteHeader() {
  return (
    <header className="sticky top-0 z-50 border-b border-transparent bg-background/75 backdrop-blur-md backdrop-saturate-[1.8]">
      <div className="mx-auto flex h-16 max-w-[1120px] items-center gap-4 px-7">
        <Link to="/" className="flex items-center gap-[9px]">
          <Logo />
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
        <a href={GITHUB_URL} target="_blank" rel="noreferrer" className={navLinkClass}>
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
          <Logo className="size-5" />
          <span className="font-mono text-[13px] font-medium text-muted-foreground">basse</span>
        </div>
        <div className="flex gap-5 font-mono text-[12.5px] text-muted-foreground">
          <Link to="/pricing" className="hover:text-foreground">
            Pricing
          </Link>
          <a href="#" className="hover:text-foreground">
            Docs
          </a>
          <a href={GITHUB_URL} target="_blank" rel="noreferrer" className="hover:text-foreground">
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
