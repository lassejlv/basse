import { Link, createFileRoute } from "@tanstack/react-router";
import { ArrowRightIcon, CheckIcon, MinusIcon, PlusIcon, ServerIcon } from "lucide-react";
import { useState } from "react";
import { DotGrid, InstallCommand, SiteFooter, SiteHeader } from "@/components/marketing";
import { cn } from "@/lib/utils";

export const Route = createFileRoute("/pricing")({
  component: PricingRoute,
});

const CLOUD_BASE_PRICE = 5;
const CLOUD_INCLUDED_SERVERS = 2;
const CLOUD_OVERAGE_PRICE = 1.5;
const CALCULATOR_MAX_SERVERS = 24;

const SELF_HOSTED_FEATURES = [
  "Unlimited servers and apps",
  "Every feature, nothing gated",
  "Runs on a €4 VPS",
  "Your data never leaves your machines",
];

const CLOUD_FEATURES = [
  "Everything in self-hosted",
  "We run and update the control plane",
  `${CLOUD_INCLUDED_SERVERS} connected servers included`,
  "Connect more anytime, priced below",
];

const FAQ = [
  {
    question: "What counts as a server?",
    answer:
      "Any machine connected to your workspace — a VPS, a dedicated box, or something under your desk. Apps, databases and deployments on it are unlimited.",
  },
  {
    question: "Is self-hosted limited?",
    answer:
      "No. Self-hosted is the same product with every feature. The only difference is who runs the dashboard: you install it on your own machine instead of us hosting it.",
  },
  {
    question: "Can I switch between the two?",
    answer:
      "Yes. Both run the same agent on your servers, so you can start on Cloud and move to self-hosted later — or the other way around.",
  },
];

function formatPrice(value: number) {
  return Number.isInteger(value) ? `$${value}` : `$${value.toFixed(2)}`;
}

function PricingRoute() {
  return (
    <>
      <SiteHeader />

      <section className="relative overflow-hidden">
        <DotGrid fadeClassName="bg-[radial-gradient(110%_110%_at_50%_0%,transparent_35%,var(--color-background)_88%)]" />

        <div className="relative mx-auto max-w-[1120px] px-7 pt-20 pb-16 lg:pt-28">
          <p className="mb-6 font-mono text-[0.7rem] font-medium uppercase tracking-[0.16em] text-muted-foreground">
            Pricing
          </p>
          <h1 className="mb-5 max-w-[16ch] text-[clamp(36px,5vw,56px)] font-semibold leading-[1.04] tracking-[-0.04em]">
            Priced per server, not per seat.
          </h1>
          <p className="max-w-[52ch] text-[17px] leading-[1.55] text-muted-foreground">
            Run the dashboard yourself for free, forever. Or let us host it for the price of a
            coffee — either way, your apps run on servers you own.
          </p>
        </div>

        <div className="relative mx-auto grid max-w-[1120px] gap-5 px-7 pb-24 lg:grid-cols-2">
          <div className="flex flex-col rounded-[14px] border bg-card/90 p-7 shadow-sm backdrop-blur-sm">
            <CardHeading title="Self-hosted" tagline="The whole platform on your own machine." />
            <Price amount="Free" detail="forever · MIT licensed" />
            <FeatureList features={SELF_HOSTED_FEATURES} />
            <div className="mt-auto">
              <InstallCommand className="bg-background" />
              <p className="mt-3 text-[13px] text-muted-foreground">
                One command installs the dashboard on any Linux machine.
              </p>
            </div>
          </div>

          <div className="flex flex-col rounded-[14px] border border-foreground/20 bg-card p-7 shadow-lg">
            <div className="flex items-start justify-between gap-3">
              <CardHeading
                title="Cloud"
                tagline="We host the dashboard. Your servers stay yours."
              />
              <span className="rounded-full border px-2.5 py-1 font-mono text-[11px] font-medium uppercase tracking-[0.1em] text-muted-foreground">
                Hosted
              </span>
            </div>
            <Price
              amount={formatPrice(CLOUD_BASE_PRICE)}
              detail={`per month · then ${formatPrice(CLOUD_OVERAGE_PRICE)} per extra server`}
            />
            <FeatureList features={CLOUD_FEATURES} />
            <ServerCalculator />
            <Link
              to="/dashboard"
              className="mt-auto inline-flex h-11 items-center justify-center gap-2 rounded-[11px] bg-primary px-[19px] text-[15px] font-medium text-primary-foreground hover:bg-primary/90 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
            >
              Start deploying
              <ArrowRightIcon className="-mr-0.5 size-4 opacity-90" strokeWidth={2.2} />
            </Link>
          </div>
        </div>
      </section>

      <section className="border-t">
        <div className="mx-auto max-w-[1120px] px-7 py-20">
          <p className="mb-10 font-mono text-[0.7rem] font-medium uppercase tracking-[0.16em] text-muted-foreground">
            Questions
          </p>
          <div className="grid grid-cols-[repeat(auto-fit,minmax(280px,1fr))] gap-x-9 gap-y-10">
            {FAQ.map((item) => (
              <div key={item.question}>
                <h3 className="mb-[7px] text-[15.5px] font-semibold">{item.question}</h3>
                <p className="max-w-[44ch] text-[14.5px] leading-[1.6] text-muted-foreground">
                  {item.answer}
                </p>
              </div>
            ))}
          </div>
        </div>
      </section>

      <SiteFooter />
    </>
  );
}

function CardHeading({ title, tagline }: { title: string; tagline: string }) {
  return (
    <div className="mb-6">
      <h2 className="text-[15.5px] font-semibold">{title}</h2>
      <p className="mt-1 text-[14.5px] text-muted-foreground">{tagline}</p>
    </div>
  );
}

function Price({ amount, detail }: { amount: string; detail: string }) {
  return (
    <div className="mb-6 flex items-baseline gap-2">
      <span className="text-[40px] font-semibold leading-none tracking-[-0.03em]">{amount}</span>
      <span className="text-sm text-muted-foreground">{detail}</span>
    </div>
  );
}

function FeatureList({ features }: { features: string[] }) {
  return (
    <ul className="mb-7 flex flex-col gap-3">
      {features.map((feature) => (
        <li key={feature} className="flex items-start gap-2.5 text-[14.5px]">
          <CheckIcon
            className="mt-[3px] size-[15px] shrink-0 text-muted-foreground"
            strokeWidth={2.4}
          />
          <span className="text-foreground/90">{feature}</span>
        </li>
      ))}
    </ul>
  );
}

function StepButton({
  onClick,
  disabled,
  label,
  icon,
}: {
  onClick: () => void;
  disabled: boolean;
  label: string;
  icon: React.ReactNode;
}) {
  return (
    <button
      type="button"
      onClick={onClick}
      disabled={disabled}
      aria-label={label}
      className="inline-flex size-7 items-center justify-center rounded-lg border text-muted-foreground hover:bg-foreground/[0.06] hover:text-foreground disabled:pointer-events-none disabled:opacity-40 focus-visible:outline-none focus-visible:ring-2 focus-visible:ring-ring"
    >
      {icon}
    </button>
  );
}

/* The Cloud price is a function of connected servers, so the card computes
   it live: tiles fill in as machines are added, included ones are solid. */
function ServerCalculator() {
  const [servers, setServers] = useState(CLOUD_INCLUDED_SERVERS);

  const overage = Math.max(0, servers - CLOUD_INCLUDED_SERVERS);
  const total = CLOUD_BASE_PRICE + overage * CLOUD_OVERAGE_PRICE;

  return (
    <div className="mb-7 rounded-[11px] border bg-background p-4">
      <div className="flex items-center justify-between gap-3">
        <span className="inline-flex items-center gap-1.5 font-mono text-xs text-muted-foreground">
          <ServerIcon className="size-3.5" />
          {servers} {servers === 1 ? "server" : "servers"}
        </span>
        <div className="flex items-center gap-1">
          <StepButton
            onClick={() => setServers((n) => Math.max(1, n - 1))}
            disabled={servers <= 1}
            label="Remove a server"
            icon={<MinusIcon className="size-3.5" strokeWidth={2.4} />}
          />
          <StepButton
            onClick={() => setServers((n) => Math.min(CALCULATOR_MAX_SERVERS, n + 1))}
            disabled={servers >= CALCULATOR_MAX_SERVERS}
            label="Add a server"
            icon={<PlusIcon className="size-3.5" strokeWidth={2.4} />}
          />
        </div>
      </div>

      <div className="mt-3 flex flex-wrap gap-1.5" aria-hidden>
        {Array.from({ length: servers }, (_, i) => (
          <span
            key={i}
            className={cn(
              "h-[18px] w-[26px] rounded-[5px] border transition-colors",
              i < CLOUD_INCLUDED_SERVERS
                ? "border-foreground/60 bg-foreground/80"
                : "border-foreground/25 bg-foreground/10",
            )}
          />
        ))}
      </div>

      <div className="mt-3 flex items-baseline justify-between border-t pt-3">
        <span className="text-[13px] text-muted-foreground">
          {overage > 0
            ? `${formatPrice(CLOUD_BASE_PRICE)} + ${overage} × ${formatPrice(CLOUD_OVERAGE_PRICE)}`
            : `${CLOUD_INCLUDED_SERVERS} servers included`}
        </span>
        <span className="font-mono text-[15px] font-semibold tabular-nums">
          {formatPrice(total)}
          <span className="ml-1 font-normal text-muted-foreground">/mo</span>
        </span>
      </div>
    </div>
  );
}
