import { Link, createFileRoute } from "@tanstack/react-router";
import {
  ArrowRightIcon,
  ArrowUpIcon,
  BoxIcon,
  CheckIcon,
  ChevronRightIcon,
  CopyIcon,
  GitBranchIcon,
  GlobeIcon,
  HardDriveIcon,
  LayersIcon,
  ScrollTextIcon,
  ServerIcon,
} from "lucide-react";
import { type ReactNode, useEffect, useRef, useState } from "react";
import { DatabaseIcon } from "@/components/database-icon";
import { StatusDot } from "@/components/deploy-status";
import { ThemeToggle } from "@/components/theme-toggle";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/")({
  component: LandingRoute,
});

const INSTALL_DISPLAY = "curl -fsSL basse.sh/install | bash";
const INSTALL_COMMAND = "curl -fsSL https://basse.sh/install | bash";

const FEATURES = [
  {
    icon: LayersIcon,
    title: "The canvas",
    body: "Every app in an environment on one surface — status is a heartbeat, not a table row.",
  },
  {
    icon: GitBranchIcon,
    title: "Push to deploy",
    body: "Connect a repo and every push builds, ships and runs. Dockerfile, Railpack, or a prebuilt image.",
  },
  {
    icon: ScrollTextIcon,
    title: "Logs that read themselves",
    body: "Build and runtime logs with severity rails, filters and search — warnings look like warnings.",
  },
  {
    icon: HardDriveIcon,
    title: "Staged changes",
    body: "Config edits batch up like a commit. Review the diff in the activity feed, deploy once.",
  },
  {
    icon: DatabaseFeatureIcon,
    title: "Managed databases",
    body: "Postgres and Redis with volumes, internal networking, and public TCP only when you ask.",
  },
  {
    icon: GlobeIcon,
    title: "Domains & HTTPS",
    body: "Point an A record and traffic routes with automatic certificates — or a managed load balancer across servers.",
  },
];

const STEPS = [
  {
    number: "01",
    title: "Connect a server",
    body: "Over SSH or the outbound agent. Basse provisions Docker and its agent for you.",
  },
  {
    number: "02",
    title: "Ship your code",
    body: "From a public repo, a private GitHub App install, or any Docker image reference.",
  },
  {
    number: "03",
    title: "It's live",
    body: "Health-checked behind your domain with HTTPS, logs and metrics streaming back.",
  },
];

const DEPLOY_LOG = [
  { time: "18:20:01", text: "Cloning github.com/acme/web (main)", level: "info" },
  { time: "18:20:04", text: "Railpack detected bun 1.3.14", level: "info" },
  { time: "18:20:12", text: "$ bun install --frozen-lockfile", level: "info" },
  { time: "18:20:48", text: "vite v6.1.0 building for production...", level: "info" },
  { time: "18:21:02", text: "warning: chunk size exceeds 500 kB", level: "warn" },
  { time: "18:21:31", text: "image push 122 MB done in 5.1s", level: "info" },
  { time: "18:21:33", text: "Deployment healthy on 1 server", level: "success" },
] as const;

function DatabaseFeatureIcon({ className }: { className?: string }) {
  return <DatabaseIcon className={className} kind="postgres" />;
}

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
            className="px-1 py-1.5 text-sm font-medium text-muted-foreground hover:text-foreground"
          >
            Pricing
          </Link>
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

      {/* Hero — the landing page opens on the product's own surface: the
          canvas dot grid with live nodes. The only color on the page is
          deploy state, same rule as the dashboard. */}
      <section className="relative overflow-hidden">
        <div
          aria-hidden
          className="absolute inset-0"
          style={{
            backgroundImage: "radial-gradient(var(--color-border) 1px, transparent 1px)",
            backgroundSize: "24px 24px",
          }}
        />
        <div
          aria-hidden
          className="absolute inset-0 bg-[radial-gradient(110%_110%_at_30%_20%,transparent_40%,var(--color-background)_92%)]"
        />

        <div className="relative mx-auto flex max-w-[1120px] flex-col items-start gap-14 px-7 pb-24 pt-24 lg:flex-row lg:items-center lg:gap-10 lg:pt-32">
          <div className="max-w-[560px]">
            <p className="mb-6 font-mono text-[0.7rem] font-medium uppercase tracking-[0.16em] text-muted-foreground">
              Self-hosted PaaS · open source
            </p>
            <h1 className="mb-6 text-[clamp(42px,6vw,68px)] font-semibold leading-[1.02] tracking-[-0.04em]">
              Deploy to servers
              <br />
              you own.
            </h1>
            <p className="mb-9 max-w-[46ch] text-[18px] leading-[1.55] text-muted-foreground">
              Connect a machine, push from Git or point at an image, and it's live with HTTPS. The
              platform experience, without the platform bill.
            </p>
            <div className="mb-8 flex flex-wrap items-center gap-[14px]">
              <Link
                to="/dashboard"
                className="inline-flex h-11 items-center gap-2 rounded-[11px] bg-primary px-[19px] text-[15px] font-medium text-primary-foreground hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
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
            <div className="inline-flex h-[42px] max-w-full items-center gap-3 rounded-[11px] border bg-card/90 pr-[5px] pl-[15px] backdrop-blur-sm">
              <span className="font-mono text-[13.5px] font-medium text-muted-foreground">$</span>
              <span className="truncate font-mono text-[13.5px] font-medium text-foreground">
                {INSTALL_DISPLAY}
              </span>
              <button
                type="button"
                onClick={copyInstall}
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
          </div>

          {/* Canvas nodes — built from the product's real vocabulary. `web`
              sits live, `api` is mid-deploy and pulses, postgres carries its
              volume tab. */}
          <div aria-hidden className="relative hidden h-[430px] flex-1 select-none lg:block">
            <HeroNode
              className="absolute left-0 top-4"
              icon={<BoxIcon className="size-4 shrink-0 text-muted-foreground/70" />}
              name="web"
              port=":3000"
              source="github.com/acme/web"
              state="Live"
              status="healthy"
            />
            <HeroNode
              className="absolute right-0 top-[150px]"
              icon={<BoxIcon className="size-4 shrink-0 text-muted-foreground/70" />}
              name="api"
              port=":8080"
              source="github.com/acme/api"
              state="Building"
              status="building"
            />
            <HeroNode
              className="absolute bottom-2 left-10"
              icon={<DatabaseIcon className="size-4 shrink-0 opacity-70" kind="postgres" />}
              name="postgres"
              port=":5432"
              source="Postgres 18"
              state="Live"
              status="healthy"
              volume="/var/lib/basse/pg"
            />
          </div>
        </div>
      </section>

      {/* How it works — a real sequence, so it earns its numbers. The log
          block is the product's log explorer, warn line and all. */}
      <section className="border-t">
        <div className="mx-auto grid max-w-[1120px] items-center gap-14 px-7 py-20 lg:grid-cols-2">
          <div>
            <p className="mb-8 font-mono text-[0.7rem] font-medium uppercase tracking-[0.16em] text-muted-foreground">
              How it works
            </p>
            <ol className="flex flex-col gap-8">
              {STEPS.map((step) => (
                <li key={step.number} className="flex gap-5">
                  <span className="mt-0.5 font-mono text-sm text-muted-foreground/60">
                    {step.number}
                  </span>
                  <div>
                    <h3 className="text-[15.5px] font-semibold">{step.title}</h3>
                    <p className="mt-1 max-w-[44ch] text-[14.5px] leading-[1.6] text-muted-foreground">
                      {step.body}
                    </p>
                  </div>
                </li>
              ))}
            </ol>
          </div>

          <div className="overflow-hidden rounded-[14px] border bg-background shadow-sm">
            <div className="flex h-[38px] items-center justify-between border-b px-[14px]">
              <span className="font-mono text-xs font-medium text-muted-foreground">
                basse up --prod
              </span>
              <span className="inline-flex items-center gap-1.5 font-mono text-[11px] text-muted-foreground">
                <StatusDot className="size-1.5" status="building" />
                deploying
              </span>
            </div>
            <div className="px-3 py-3 font-mono text-[12.5px] leading-[1.9]">
              {DEPLOY_LOG.map((line) => (
                <div key={line.time} className="flex gap-2.5">
                  <span
                    className={cn(
                      "w-0.5 shrink-0 self-stretch rounded-full",
                      line.level === "warn"
                        ? "bg-warning"
                        : line.level === "success"
                          ? "bg-success"
                          : "bg-border",
                    )}
                  />
                  <span className="shrink-0 select-none text-muted-foreground/50">{line.time}</span>
                  <span
                    className={cn(
                      "min-w-0 truncate",
                      line.level === "warn"
                        ? "text-warning-foreground"
                        : line.level === "success"
                          ? "text-success-foreground"
                          : "text-foreground/85",
                    )}
                  >
                    {line.text}
                  </span>
                </div>
              ))}
              <div className="mt-1 flex gap-2.5">
                <span className="w-0.5 shrink-0 self-stretch rounded-full bg-success" />
                <span className="shrink-0 select-none text-muted-foreground/50">18:21:34</span>
                <span className="min-w-0 truncate text-foreground">
                  ✓ live at{" "}
                  <span className="underline underline-offset-[3px]">app-prod.basse.network</span>
                </span>
              </div>
            </div>
            <div className="border-t px-3 py-1.5 font-mono text-[11px] text-muted-foreground">
              8 lines · 1 warn
            </div>
          </div>
        </div>
      </section>

      <section className="border-t">
        <div className="mx-auto max-w-[1120px] px-7 py-20">
          <p className="mb-10 font-mono text-[0.7rem] font-medium uppercase tracking-[0.16em] text-muted-foreground">
            What's in the box
          </p>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(260px,1fr))] gap-x-9 gap-y-10">
            {FEATURES.map((feature) => (
              <div key={feature.title}>
                <feature.icon className="mb-3 size-[18px] text-muted-foreground" />
                <h3 className="mb-[7px] text-[15.5px] font-semibold">{feature.title}</h3>
                <p className="max-w-[40ch] text-[14.5px] leading-[1.6] text-muted-foreground">
                  {feature.body}
                </p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <section className="border-t">
        <div className="mx-auto max-w-[680px] px-7 pt-[72px] pb-[88px] text-center">
          <div className="mb-6 inline-flex items-center gap-2 font-mono text-xs text-muted-foreground">
            <ServerIcon className="size-3.5" />
            Runs on a €4 VPS
          </div>
          <h2 className="mb-[14px] text-[clamp(26px,3.6vw,34px)] font-semibold leading-[1.1] tracking-[-0.025em]">
            Ship your first app in minutes
          </h2>
          <p className="mx-auto mb-7 max-w-[42ch] text-base leading-[1.55] text-muted-foreground">
            Install the CLI, connect a server, deploy. Free and open source, MIT licensed.
          </p>
          <div className="flex flex-wrap items-center justify-center gap-[14px]">
            <Link
              to="/dashboard"
              className="inline-flex h-11 items-center gap-2 rounded-[11px] bg-primary px-[19px] text-[15px] font-medium text-primary-foreground hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
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
        </div>
      </section>

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
    </>
  );
}

function HeroNode({
  name,
  source,
  state,
  port,
  status,
  icon,
  volume,
  className,
}: {
  name: string;
  source: string;
  state: string;
  port: string;
  status: "healthy" | "building";
  icon: ReactNode;
  volume?: string;
  className?: string;
}) {
  return (
    <div className={cn("w-[248px]", className)}>
      <div className="relative z-10 rounded-xl border bg-card p-3.5 shadow-lg">
        <div className="flex items-center gap-2.5">
          <StatusDot status={status} />
          <span className="min-w-0 flex-1 truncate text-sm font-medium">{name}</span>
          {icon}
        </div>
        <p className="mt-1.5 truncate font-mono text-xs text-muted-foreground">{source}</p>
        <div className="mt-2.5 flex items-center justify-between gap-2">
          <span className="text-xs text-muted-foreground">{state}</span>
          <span className="font-mono text-[11px] text-muted-foreground/70">{port}</span>
        </div>
      </div>
      {volume ? (
        <div className="-mt-2 mx-4 flex items-center gap-1.5 rounded-b-lg border bg-card px-2.5 pt-[13px] pb-1.5 font-mono text-[11px] text-muted-foreground">
          <HardDriveIcon className="size-3 shrink-0" />
          <span className="truncate">{volume}</span>
        </div>
      ) : null}
    </div>
  );
}
