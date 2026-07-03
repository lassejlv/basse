import { createFileRoute } from "@tanstack/react-router";
import {
  BoxIcon,
  ChevronRightIcon,
  GitBranchIcon,
  GlobeIcon,
  HardDriveIcon,
  LayersIcon,
  ScrollTextIcon,
  ServerIcon,
} from "lucide-react";
import type { ReactNode } from "react";
import { DatabaseIcon } from "@/components/database-icon";
import { StatusDot } from "@/components/deploy-status";
import {
  DeployCta,
  DotGrid,
  Eyebrow,
  GITHUB_URL,
  InstallCommand,
  SiteFooter,
  SiteHeader,
} from "@/components/marketing";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/")({
  component: LandingRoute,
});

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

function DatabaseFeatureIcon({ className }: { className?: string }) {
  return <DatabaseIcon className={className} kind="postgres" />;
}

function LandingRoute() {
  return (
    <>
      <SiteHeader />

      {/* Hero — the landing page opens on the product's own surface: the
          canvas dot grid with live nodes. The only color on the page is
          deploy state, same rule as the dashboard. */}
      <section className="relative overflow-hidden">
        <DotGrid fadeClassName="bg-[radial-gradient(110%_110%_at_30%_20%,transparent_40%,var(--color-background)_92%)]" />

        <div className="relative mx-auto flex max-w-[1120px] flex-col items-start gap-14 px-7 pb-24 pt-24 lg:flex-row lg:items-center lg:gap-10 lg:pt-32">
          <div className="max-w-[560px]">
            <Eyebrow className="mb-6">Self-hosted PaaS · open source</Eyebrow>
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
              <DeployCta label="Deploy your first app" />
              <a
                href="#"
                className="inline-flex h-11 items-center gap-1.5 px-2 text-[15px] font-medium text-foreground hover:text-muted-foreground"
              >
                Read the docs
                <ChevronRightIcon className="size-[15px] opacity-60" />
              </a>
            </div>
            <InstallCommand className="w-fit bg-card/90 backdrop-blur-sm" />
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

      <section className="border-t">
        <div className="mx-auto max-w-[1120px] px-7 py-20">
          <Eyebrow className="mb-10">How it works</Eyebrow>
          <ol className="grid gap-10 sm:grid-cols-3">
            {STEPS.map((step) => (
              <li key={step.number}>
                <span className="font-mono text-sm text-muted-foreground/60">{step.number}</span>
                <h3 className="mt-2 text-[15.5px] font-semibold">{step.title}</h3>
                <p className="mt-1 max-w-[44ch] text-[14.5px] leading-[1.6] text-muted-foreground">
                  {step.body}
                </p>
              </li>
            ))}
          </ol>
        </div>
      </section>

      <section className="border-t">
        <div className="mx-auto max-w-[1120px] px-7 py-20">
          <Eyebrow className="mb-10">What's in the box</Eyebrow>
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
            <DeployCta label="Deploy now" />
            <a
              href={GITHUB_URL}
              target="_blank"
              rel="noreferrer"
              className="inline-flex h-11 items-center gap-1.5 px-2 text-[15px] font-medium text-foreground hover:text-muted-foreground"
            >
              View on GitHub
            </a>
          </div>
        </div>
      </section>

      <SiteFooter />
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
