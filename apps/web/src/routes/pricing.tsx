import { createFileRoute } from "@tanstack/react-router";
import { CheckIcon } from "lucide-react";
import {
  DeployCta,
  DotGrid,
  Eyebrow,
  InstallCommand,
  SiteFooter,
  SiteHeader,
} from "@/components/marketing";

export const Route = createFileRoute("/pricing")({
  component: PricingRoute,
});

const SELF_HOSTED_FEATURES = [
  "Unlimited servers and apps",
  "Every feature, nothing gated",
  "Runs on a €4 VPS",
  "Your data never leaves your machines",
];

const CLOUD_FEATURES = [
  "Everything in self-hosted",
  "We run and update the control plane",
  "2 connected servers included",
  "$1.50 per month for each extra server",
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

function PricingRoute() {
  return (
    <>
      <SiteHeader />

      <section className="relative overflow-hidden">
        <DotGrid fadeClassName="bg-[radial-gradient(110%_110%_at_50%_0%,transparent_35%,var(--color-background)_88%)]" />

        <div className="relative mx-auto max-w-[1120px] px-7 pt-20 pb-16 lg:pt-28">
          <Eyebrow className="mb-6">Pricing</Eyebrow>
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
            <Price amount="$5" detail="per month · 2 servers included" />
            <FeatureList features={CLOUD_FEATURES} />
            <DeployCta label="Start deploying" className="mt-auto justify-center" />
          </div>
        </div>
      </section>

      <section className="border-t">
        <div className="mx-auto max-w-[1120px] px-7 py-20">
          <Eyebrow className="mb-10">Questions</Eyebrow>
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
