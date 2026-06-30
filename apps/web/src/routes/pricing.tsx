import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowUpIcon, CheckIcon, MinusIcon, PlusIcon } from "lucide-react";
import { useState } from "react";
import { ThemeToggle } from "@/components/theme-toggle";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";

export const Route = createFileRoute("/pricing")({
  component: PricingRoute,
});

const GITHUB_URL = "https://github.com/lassejlv/basse";

// Cloud plan: a $5 base that includes 2 connected servers, then a flat fee per
// additional server. The stepper below recomputes the total live.
const BASE_PRICE = 5;
const INCLUDED_SERVERS = 2;
const PRICE_PER_EXTRA = 1.5;
const MIN_SERVERS = 1;
const MAX_SERVERS = 50;

function money(value: number): string {
  return `$${value.toFixed(2)}`;
}

const OPEN_SOURCE_FEATURES = [
  "Unlimited servers & apps",
  "Git & Docker deploys",
  "Logs, metrics & rollbacks",
  "Community support",
];

const CLOUD_FEATURES = [
  "Everything in open source",
  "Managed control plane & backups",
  "One-click updates & priority support",
];

const FAQS = [
  {
    q: "What counts as a server?",
    a: "Any machine you connect over SSH. Billing is based on how many are connected — apps and deployments are unlimited.",
  },
  {
    q: "What if I run two or fewer?",
    a: "You pay only the $5 base — your first two servers are included. Each server after that is a flat $1.50/mo.",
  },
  {
    q: "Can I just self-host?",
    a: "Always. basse is open source and free to run on your own servers, with every feature and no limits.",
  },
  {
    q: "Can I cancel anytime?",
    a: "Yes. Billing is monthly and prorated — add or remove servers whenever you like and the price follows.",
  },
];

function PricingRoute() {
  const [servers, setServers] = useState(4);

  const extra = Math.max(0, servers - INCLUDED_SERVERS);
  const overage = extra * PRICE_PER_EXTRA;
  const total = BASE_PRICE + overage;
  const serversLabel = `${servers} ${servers === 1 ? "server" : "servers"}`;

  return (
    <div className="min-h-svh bg-background text-foreground">
      <SiteHeader />

      <section className="mx-auto max-w-[680px] px-7 pt-26 pb-12 text-center">
        <div className="mb-5 font-mono text-muted-foreground text-sm tracking-wide">Pricing</div>
        <h1 className="text-[clamp(38px,5.4vw,56px)] font-semibold leading-[1.04] tracking-[-0.035em]">
          Pay for the servers you run
        </h1>
        <p className="mx-auto mt-4 max-w-[48ch] text-lg text-muted-foreground leading-relaxed">
          No seats, no per-app fees. Self-host for free, or let basse manage it — billed by the
          number of connected servers.
        </p>
      </section>

      <section className="mx-auto max-w-[880px] px-7">
        <div className="grid items-stretch gap-[18px] sm:grid-cols-2">
          {/* Open source */}
          <div className="flex flex-col rounded-[18px] border border-border bg-card p-7">
            <div className="mb-3.5 flex items-center gap-2.5">
              <span className="font-semibold text-base">Open source</span>
              <Badge className="font-mono text-muted-foreground" variant="outline">
                self-host
              </Badge>
            </div>
            <div className="mb-1.5 flex items-baseline gap-1.5">
              <span className="font-semibold text-[44px] leading-none tracking-[-0.03em]">$0</span>
              <span className="text-[15px] text-muted-foreground">/ forever</span>
            </div>
            <p className="mb-[22px] text-muted-foreground text-sm leading-relaxed">
              Run basse on your own machines. Every feature, no server limits.
            </p>
            <ul className="mb-6 flex flex-col gap-2.5">
              {OPEN_SOURCE_FEATURES.map((feature) => (
                <Feature key={feature}>{feature}</Feature>
              ))}
            </ul>
            <div className="flex-1" />
            <Button
              className="h-[42px] w-full rounded-[11px] text-[14.5px]"
              render={<a href={GITHUB_URL} rel="noreferrer" target="_blank" />}
              variant="outline"
            >
              <GitHubMark />
              View on GitHub
            </Button>
          </div>

          {/* Cloud */}
          <div className="flex flex-col rounded-[18px] border-[1.5px] border-primary bg-card p-7 shadow-[0_8px_30px_rgba(0,0,0,0.08)]">
            <div className="mb-3.5 flex items-center gap-2.5">
              <span className="font-semibold text-base">Cloud</span>
              <Badge className="font-mono">managed</Badge>
            </div>
            <div className="mb-1 flex items-baseline gap-1.5">
              <span className="font-semibold text-[44px] leading-none tracking-[-0.03em] tabular-nums">
                {money(total)}
              </span>
              <span className="text-[15px] text-muted-foreground">/ mo</span>
            </div>
            <p className="mb-5 text-muted-foreground text-sm">
              for {servers} connected {servers === 1 ? "server" : "servers"}
            </p>

            <div className="mb-[22px] rounded-[14px] border border-border p-4">
              <div className="mb-3.5 flex items-center justify-between gap-3">
                <span className="font-medium text-[13.5px]">Servers</span>
                <div className="inline-flex items-stretch overflow-hidden rounded-[10px] border border-input bg-background">
                  <button
                    aria-label="Remove server"
                    className="flex h-9 w-[38px] items-center justify-center text-foreground transition hover:bg-accent disabled:pointer-events-none disabled:opacity-40"
                    disabled={servers <= MIN_SERVERS}
                    onClick={() => setServers((value) => Math.max(MIN_SERVERS, value - 1))}
                    type="button"
                  >
                    <MinusIcon className="size-[15px]" />
                  </button>
                  <div className="flex min-w-[104px] items-center justify-center border-border border-x px-2 font-semibold text-sm tabular-nums">
                    {serversLabel}
                  </div>
                  <button
                    aria-label="Add server"
                    className="flex h-9 w-[38px] items-center justify-center text-foreground transition hover:bg-accent disabled:pointer-events-none disabled:opacity-40"
                    disabled={servers >= MAX_SERVERS}
                    onClick={() => setServers((value) => Math.min(MAX_SERVERS, value + 1))}
                    type="button"
                  >
                    <PlusIcon className="size-[15px]" />
                  </button>
                </div>
              </div>
              <CostRow label="Base · 2 servers included" value={money(BASE_PRICE)} />
              {extra > 0 ? (
                <CostRow
                  label={`${extra} extra ${extra === 1 ? "server" : "servers"} × $1.50`}
                  value={money(overage)}
                />
              ) : null}
              <div className="my-2 h-px bg-border" />
              <div className="flex items-center justify-between py-0.5 font-semibold text-sm">
                <span>Total</span>
                <span className="font-mono tabular-nums">{money(total)} / mo</span>
              </div>
            </div>

            <ul className="mb-6 flex flex-col gap-2.5">
              {CLOUD_FEATURES.map((feature) => (
                <Feature key={feature}>{feature}</Feature>
              ))}
            </ul>
            <div className="flex-1" />
            <Button
              className="h-[42px] w-full rounded-[11px] text-[14.5px]"
              render={<Link to="/signup" />}
            >
              Start with {serversLabel}
            </Button>
            <p className="mt-3 text-center font-mono text-muted-foreground text-xs">
              billed monthly · cancel anytime
            </p>
          </div>
        </div>
      </section>

      <section className="mx-auto max-w-[880px] px-7 pt-16 pb-6">
        <div className="border-border border-t pt-12">
          <h2 className="mb-7 text-center font-semibold text-[22px] tracking-[-0.015em]">
            Questions
          </h2>
          <div className="grid gap-x-10 gap-y-8 sm:grid-cols-2">
            {FAQS.map((faq) => (
              <div key={faq.q}>
                <div className="mb-1.5 font-semibold text-[15px]">{faq.q}</div>
                <div className="text-muted-foreground text-sm leading-relaxed">{faq.a}</div>
              </div>
            ))}
          </div>
        </div>
      </section>

      <footer className="mt-12 border-border border-t">
        <div className="mx-auto flex max-w-[1040px] flex-wrap items-center justify-between gap-4 px-7 py-6">
          <div className="flex items-center gap-2.5">
            <Logo className="size-5 rounded-md" iconClass="size-[11px]" />
            <span className="font-mono text-[13px] text-muted-foreground">basse</span>
          </div>
          <div className="flex gap-5 font-mono text-[12.5px] text-muted-foreground">
            <a className="transition hover:text-foreground" href={GITHUB_URL} rel="noreferrer" target="_blank">
              GitHub
            </a>
            <span>MIT licensed</span>
          </div>
        </div>
      </footer>
    </div>
  );
}

function SiteHeader() {
  return (
    <header
      className="sticky top-0 z-50 backdrop-blur-md backdrop-saturate-150"
      style={{ background: "color-mix(in srgb, var(--background) 76%, transparent)" }}
    >
      <div className="mx-auto flex h-16 max-w-[1040px] items-center gap-4 px-7">
        <Link className="flex items-center gap-2.5 text-foreground" to="/">
          <Logo />
          <span className="font-semibold text-base tracking-tight">basse</span>
        </Link>
        <div className="flex-1" />
        <a
          className="hidden px-1 py-1.5 font-medium text-muted-foreground text-sm transition hover:text-foreground sm:inline-flex"
          href={GITHUB_URL}
          rel="noreferrer"
          target="_blank"
        >
          Docs
        </a>
        <span className="px-1 py-1.5 font-medium text-foreground text-sm">Pricing</span>
        <a
          className="hidden px-1 py-1.5 font-medium text-muted-foreground text-sm transition hover:text-foreground sm:inline-flex"
          href={GITHUB_URL}
          rel="noreferrer"
          target="_blank"
        >
          GitHub
        </a>
        <ThemeToggle />
        <Button render={<Link to="/signup" />} size="sm">
          Deploy
        </Button>
      </div>
    </header>
  );
}

function Logo({ className, iconClass }: { className?: string; iconClass?: string }) {
  return (
    <span
      className={`inline-flex items-center justify-center bg-primary text-primary-foreground ${
        className ?? "size-6 rounded-[7px]"
      }`}
    >
      <ArrowUpIcon className={iconClass ?? "size-3.5"} strokeWidth={2.6} />
    </span>
  );
}

function GitHubMark() {
  return (
    <svg
      fill="none"
      height="16"
      stroke="currentColor"
      strokeLinecap="round"
      strokeLinejoin="round"
      strokeWidth={2}
      viewBox="0 0 24 24"
      width="16"
    >
      <path d="M15 22v-4a4.8 4.8 0 0 0-1-3.5c3 0 6-2 6-5.5.08-1.25-.27-2.48-1-3.5.28-1.15.28-2.35 0-3.5 0 0-1 0-3 1.5-2.64-.5-5.36-.5-8 0C5 2 4 2 4 2c-.3 1.15-.3 2.35 0 3.5A5.4 5.4 0 0 0 3 9c0 3.5 3 5.5 6 5.5-.39.49-.68 1.05-.85 1.65-.17.6-.22 1.23-.15 1.85v4" />
      <path d="M9 18c-4.51 2-5-2-7-2" />
    </svg>
  );
}

function Feature({ children }: { children: React.ReactNode }) {
  return (
    <li className="flex items-center gap-2.5 text-sm">
      <CheckIcon className="size-[15px] shrink-0 text-foreground/55" strokeWidth={2.4} />
      {children}
    </li>
  );
}

function CostRow({ label, value }: { label: string; value: string }) {
  return (
    <div className="flex items-center justify-between py-[5px] text-[13px] text-muted-foreground">
      <span>{label}</span>
      <span className="font-mono text-foreground tabular-nums">{value}</span>
    </div>
  );
}
