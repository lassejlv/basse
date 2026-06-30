import { Link, createFileRoute } from "@tanstack/react-router";
import {
  ArrowRightIcon,
  ArrowUpIcon,
  CheckIcon,
  ChevronRightIcon,
  CopyIcon,
} from "lucide-react";
import { useEffect, useRef, useState } from "react";
import { ThemeToggle } from "@/components/theme-toggle";

export const Route = createFileRoute("/")({
  component: LandingRoute,
});

const INSTALL_DISPLAY = "curl -fsSL basse.dev/install | sh";
const INSTALL_COMMAND = "curl -fsSL https://basse.dev/install | sh";

const FEATURES = [
  {
    title: "Push to deploy",
    body: "Connect a repo and every push builds, ships and runs — no pipeline to write.",
  },
  {
    title: "Any container",
    body: "Dockerfile, buildpack or a prebuilt image. If it runs in a container, it deploys.",
  },
  {
    title: "Yours to own",
    body: "Self-hosted on your servers. Open source. No lock-in, no surprise bills.",
  },
];

const TERMINAL_LINES = [
  { dot: "$", dotClass: "text-success-foreground", text: "basse up --prod" },
  { prefix: "→ detected ", strong: "Bun", suffix: " · building image" },
  { prefix: "→ pushing 7 layers to ", strong: "fra1" },
  { prefix: "→ health check ", ok: "passed", suffix: " on :3000" },
];

function LandingRoute() {
  const [copied, setCopied] = useState(false);
  const timeoutRef = useRef<ReturnType<typeof setTimeout> | null>(null);

  useEffect(() => () => clearTimeout(timeoutRef.current ?? undefined), []);

  function copyInstall() {
    navigator.clipboard?.writeText(INSTALL_COMMAND).catch(() => {});
    setCopied(true);
    clearTimeout(timeoutRef.current ?? undefined);
    timeoutRef.current = setTimeout(() => setCopied(false), 1600);
  }

  return (
    <>
      <header className="sticky top-0 z-50 bg-background/75 backdrop-blur-md backdrop-saturate-[1.8]">
        <div className="mx-auto flex h-16 max-w-[1040px] items-center gap-4 px-7">
          <Link to="/" className="flex items-center gap-[9px]">
            <span className="inline-flex size-6 items-center justify-center rounded-[7px] bg-primary text-primary-foreground">
              <ArrowUpIcon className="size-3.5" strokeWidth={2.6} />
            </span>
            <span className="text-base font-semibold tracking-[-0.01em]">basse</span>
          </Link>
          <div className="flex-1" />
          <a
            href="#"
            className="px-1 py-1.5 text-sm font-medium text-muted-foreground hover:text-foreground"
          >
            Docs
          </a>
          <a
            href="#"
            className="px-1 py-1.5 text-sm font-medium text-muted-foreground hover:text-foreground"
          >
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

      <section className="mx-auto max-w-[720px] px-7 pt-[140px] pb-14 text-center">
        <div className="mb-7 font-mono text-[13px] font-medium tracking-[0.04em] text-muted-foreground">
          Self-hosted PaaS · open source
        </div>
        <h1 className="mb-6 text-[clamp(46px,6.8vw,76px)] font-semibold leading-none tracking-[-0.04em]">
          Just deploy it.
        </h1>
        <p className="mx-auto mb-10 max-w-[46ch] text-[19px] leading-[1.55] text-muted-foreground">
          Anything that fits in a container, running on servers you own. Straight from Git or a
          Docker image.
        </p>
        <div className="mb-9 flex flex-wrap items-center justify-center gap-[14px]">
          <Link
            to="/dashboard"
            className="inline-flex h-11 items-center gap-2 rounded-[11px] bg-primary px-[19px] text-[15px] font-medium text-primary-foreground hover:bg-primary/90"
          >
            Deploy your first app
            <ArrowRightIcon className="-mr-0.5 size-4 opacity-90" strokeWidth={2.2} />
          </Link>
          <a
            href="#"
            className="inline-flex h-11 items-center gap-1.5 px-2 text-[15px] font-medium text-foreground hover:text-muted-foreground"
          >
            Read the docs
            <ChevronRightIcon className="size-[15px] opacity-60" />
          </a>
        </div>
        <div className="inline-flex h-[42px] items-center gap-3 rounded-[11px] border bg-card pr-[5px] pl-[15px]">
          <span className="font-mono text-[13.5px] font-medium text-muted-foreground">$</span>
          <span className="font-mono text-[13.5px] font-medium text-foreground">
            {INSTALL_DISPLAY}
          </span>
          <button
            type="button"
            onClick={copyInstall}
            aria-label="Copy install command"
            className="inline-flex h-8 items-center gap-1.5 rounded-lg px-2.5 text-[13px] font-medium text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground"
          >
            {copied ? (
              <CheckIcon className="size-3.5 text-success-foreground" strokeWidth={2.4} />
            ) : (
              <CopyIcon className="size-3.5 opacity-80" />
            )}
            {copied ? "Copied" : "Copy"}
          </button>
        </div>
      </section>

      <section className="mx-auto max-w-[680px] px-7 pt-2 pb-10">
        <div className="overflow-hidden rounded-[14px] border bg-code">
          <div className="flex h-[38px] items-center gap-[9px] border-b px-[14px]">
            <span className="flex gap-1.5">
              {["one", "two", "three"].map((dot) => (
                <span key={dot} className="size-2.5 rounded-full bg-input" />
              ))}
            </span>
            <span className="flex-1 text-center font-mono text-xs font-medium text-muted-foreground">
              basse up
            </span>
          </div>
          <div className="overflow-x-auto px-5 py-[18px] font-mono text-[13px] font-medium leading-[2]">
            {TERMINAL_LINES.map((line) => (
              <div key={line.text ?? line.prefix}>
                {line.dot ? (
                  <span className={line.dotClass}>{line.dot} </span>
                ) : null}
                {line.text ? <span className="text-foreground">{line.text}</span> : null}
                {line.prefix ? <span className="text-muted-foreground">{line.prefix}</span> : null}
                {line.strong ? <span className="text-foreground">{line.strong}</span> : null}
                {line.ok ? <span className="text-success-foreground">{line.ok}</span> : null}
                {line.suffix ? <span className="text-muted-foreground">{line.suffix}</span> : null}
              </div>
            ))}
            <div className="text-foreground">
              <span className="text-success-foreground">✓</span> deployed{" "}
              <span className="text-foreground">v1.4.2</span> →{" "}
              <span className="text-foreground underline underline-offset-[3px]">app.basse.dev</span>
            </div>
          </div>
        </div>
      </section>

      <section className="mx-auto max-w-[940px] px-7 py-12">
        <div className="grid grid-cols-[repeat(auto-fit,minmax(220px,1fr))] gap-9 border-t pt-11">
          {FEATURES.map((feature) => (
            <div key={feature.title}>
              <div className="mb-[9px] text-[15.5px] font-semibold">{feature.title}</div>
              <div className="text-[14.5px] leading-[1.6] text-muted-foreground">{feature.body}</div>
            </div>
          ))}
        </div>
      </section>

      <section className="mx-auto max-w-[680px] border-t px-7 pt-[72px] pb-[88px] text-center">
        <h2 className="mb-[14px] text-[clamp(26px,3.6vw,34px)] font-semibold leading-[1.1] tracking-[-0.025em]">
          Ship your first app in minutes
        </h2>
        <p className="mx-auto mb-7 max-w-[42ch] text-base leading-[1.55] text-muted-foreground">
          Install the CLI, connect a server, deploy. Free and open source.
        </p>
        <div className="flex flex-wrap items-center justify-center gap-[14px]">
          <Link
            to="/dashboard"
            className="inline-flex h-11 items-center gap-2 rounded-[11px] bg-primary px-[19px] text-[15px] font-medium text-primary-foreground hover:bg-primary/90"
          >
            Deploy now
            <ArrowRightIcon className="-mr-0.5 size-4 opacity-90" strokeWidth={2.2} />
          </Link>
          <a
            href="#"
            className="inline-flex h-11 items-center gap-1.5 px-2 text-[15px] font-medium text-foreground hover:text-muted-foreground"
          >
            View on GitHub
          </a>
        </div>
      </section>

      <footer className="border-t">
        <div className="mx-auto flex max-w-[1040px] flex-wrap items-center justify-between gap-4 px-7 py-6">
          <div className="flex items-center gap-[9px]">
            <span className="inline-flex size-5 items-center justify-center rounded-md bg-primary text-primary-foreground">
              <ArrowUpIcon className="size-[11px]" strokeWidth={2.6} />
            </span>
            <span className="font-mono text-[13px] font-medium text-muted-foreground">basse</span>
          </div>
          <div className="flex gap-5 font-mono text-[12.5px] text-muted-foreground">
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
    </>
  );
}
