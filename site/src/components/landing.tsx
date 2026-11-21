import type { ReactNode } from "react"
import { Button } from "@/components/ui/button"
import { HeroScene } from "@/components/hero-scene"
import { InstallCommand } from "@/components/install"
import { KeyLink } from "@/components/key-hint"
import { Panel } from "@/components/panel"
import { SectionHead } from "@/components/section-head"
import { TuiCompact, TuiOverview } from "@/components/tui"
import { brand, installCommand, links } from "@/config"
import { useCopy } from "@/lib/use-copy"
import { useShortcuts } from "@/lib/use-shortcuts"

const features = [
  { title: "proxy", art: "lighthouse", heading: "Caddy, automatic HTTPS", command: `${brand.name} domain add web shop.acme.dk` },
  { title: "databases", art: "tanks", heading: "Postgres 18, backed up to S3", command: `${brand.name} db schedule postgres --hour 3 --keep 7` },
  { title: "instance backups", art: "anchor", heading: "Restore the whole instance", command: `${brand.name} instance backup --storage offsite` },
] as const

const rest = [
  ["projects", "any environment"],
  ["sources", "git, image or database"],
  ["deploys", "zero downtime"],
  ["logs", "live"],
  ["secrets", "encrypted at rest"],
  ["input", "mouse + keys"],
] as const

const specs = [
  ["binaries", `${brand.daemon} + ${brand.name}, static binaries`],
  ["store", "Postgres 18"],
  ["servers", "Linux x86_64 and arm64"],
  ["your laptop", "macOS or Linux CLI"],
  ["license", brand.license],
] as const

const container = "mx-auto w-full max-w-[1440px] px-5 md:px-16"

function Prompt() {
  return <span aria-hidden="true" className="text-mist select-none">$</span>
}

function Line({ children }: { children: ReactNode }) {
  return <div className="flex gap-2.5 whitespace-pre">{children}</div>
}

export function Landing({ version }: { version: string }) {
  const { state, copy } = useCopy(installCommand)
  useShortcuts({
    c: copy,
    g: () => window.location.assign(links.github),
    d: () => window.location.assign(links.docs),
    i: () => {
      const target = document.getElementById("install")
      target?.scrollIntoView({ block: "center" })
      target?.querySelector("button")?.focus({ preventScroll: true })
    },
    f: () => document.getElementById("features")?.scrollIntoView(),
    h: () => document.getElementById("how")?.scrollIntoView(),
  })

  return (
    <>
      <a href="#main" className="sr-only focus:not-sr-only focus:absolute focus:top-3 focus:left-3 focus:z-10 focus:bg-quay focus:px-2">
        Skip to content
      </a>
      <header className="border-b border-line">
        <div className={`${container} flex h-[52px] items-center justify-between md:h-14`}>
          <p className="flex items-center gap-2.5">
            <span className="font-medium text-amber">{brand.name}</span>
            <span aria-hidden="true" className="text-mist">›</span>
            <span>home</span>
            <span className="hidden text-mist md:inline">v{version} · {brand.license}</span>
          </p>
          <nav aria-label="Primary" className="flex items-center gap-[18px] md:gap-6">
            <KeyLink keyName="f" href="#features" className="hidden md:inline-flex">features</KeyLink>
            <KeyLink keyName="h" href="#how" className="hidden md:inline-flex">how it works</KeyLink>
            <KeyLink keyName="d" href={links.docs}>docs</KeyLink>
            <KeyLink keyName="g" href={links.github}>github ↗</KeyLink>
          </nav>
        </div>
      </header>

      <main id="main">
        <section aria-labelledby="hero-title" className="relative h-[900px]">
          <HeroScene />
          <div className={`${container} relative flex flex-col gap-6 pt-12 md:gap-9 md:pt-24`}>
            <div className="flex flex-col gap-6 md:gap-7">
              <h1 id="hero-title" className="max-w-[920px] text-[30px] leading-[38px] font-medium tracking-[-0.03em] md:text-5xl md:leading-[58px]">
                {brand.tagline}
              </h1>
              <p className="text-sm leading-[23px] text-mist md:text-[15px] md:leading-[26px]">{brand.summary}</p>
            </div>
            <div id="install" className="md:w-[980px]">
              <InstallCommand state={state} onCopy={copy} />
            </div>
            <div className="flex gap-3">
              <Button variant="outline" asChild className="flex-1 md:flex-none">
                <a href={links.github} aria-keyshortcuts="g"><kbd className="text-amber">g</kbd>github ↗</a>
              </Button>
              <Button variant="outline" asChild className="flex-1 md:flex-none">
                <a href={links.docs} aria-keyshortcuts="d"><kbd className="text-amber">d</kbd>read the docs</a>
              </Button>
            </div>
          </div>
        </section>

        <section aria-labelledby="tui" className={`${container} flex flex-col gap-7 pt-[88px] md:gap-12 md:pt-[120px]`}>
          <SectionHead index="01" label="the TUI" id="tui" title="Every server and deploy on one screen." />
          <figure className="self-center overflow-hidden rounded-[6px] border border-line max-[1359px]:w-full min-[1360px]:rounded-lg">
            <div aria-hidden="true" className="relative flex h-6 items-center gap-1.5 border-b border-line bg-hull px-2.5 min-[1360px]:h-9 min-[1360px]:gap-2 min-[1360px]:px-3.5">
              {[0, 1, 2].map((i) => (
                <span key={i} className="size-2 rounded-full bg-line min-[1360px]:size-2.5" />
              ))}
              <span className="absolute inset-x-0 hidden text-center text-xs text-mist min-[1360px]:block">root@prod-eu-1: {brand.name}</span>
            </div>
            <div className="hidden min-[1360px]:block"><TuiOverview version={version} /></div>
            <div className="min-[1360px]:hidden"><TuiCompact version={version} /></div>
            <figcaption className="sr-only">The {brand.name} TUI showing a project's services, the selected service and recent activity.</figcaption>
          </figure>
        </section>

        <section id="features" aria-labelledby="features-title" className={`${container} flex flex-col gap-6 pt-[88px] md:gap-12 md:pt-36`}>
          <SectionHead index="02" label="what it does" id="features-title" title="Build, run, route and back up." />
          <div className="flex flex-col gap-6 md:flex-row">
            {features.map((f) => (
              <Panel key={f.title} title={f.title} className="flex-1">
                <div className="flex items-center gap-4 px-3.5 pt-[18px] pb-3.5 md:flex-col md:items-stretch md:gap-5 md:px-5 md:pt-[22px] md:pb-5">
                  <img src={`/dither/${f.art}-small.png`} alt="" width={100} height={70} loading="lazy" className="pixelated shrink-0 md:hidden" />
                  <img src={`/dither/${f.art}.png`} alt="" width={200} height={140} loading="lazy" className="pixelated hidden shrink-0 md:block" />
                  <div className="flex min-w-0 flex-1 flex-col gap-1.5 md:gap-5">
                    <h3 className="text-sm leading-5 font-medium md:text-[17px] md:leading-6">{f.heading}</h3>
                    <p className="flex gap-2.5 truncate text-xs text-mist md:border-t md:border-line md:pt-3.5 md:text-[13px] md:text-fog">
                      <span aria-hidden="true" className="hidden text-mist select-none md:inline">$</span>
                      <code className="truncate">{f.command}</code>
                    </p>
                  </div>
                </div>
              </Panel>
            ))}
          </div>
          <Panel title="and the rest">
            <dl className="grid grid-cols-1 px-3.5 pt-2 pb-1 md:grid-flow-col md:grid-cols-2 md:grid-rows-3 md:gap-x-12 md:px-6 md:pt-3.5 md:pb-2.5">
              {rest.map(([term, detail]) => (
                <div key={term} className="flex justify-between gap-3 border-b border-line py-2.5 md:justify-start md:gap-6 md:py-3.5 ">
                  <dt className="md:w-[180px] md:shrink-0">{term}</dt>
                  <dd className="text-right text-mist md:text-left">{detail}</dd>
                </div>
              ))}
            </dl>
          </Panel>
        </section>

        <section id="how" aria-labelledby="how-title" className={`${container} flex flex-col gap-6 pt-[88px] md:gap-12 md:pt-36`}>
          <SectionHead index="03" label="how it works" id="how-title" title="Bare server to HTTPS in three commands." />
          <ol className="flex flex-col gap-6 text-xs leading-[19px] md:flex-row md:text-[13px] md:leading-5">
            <li className="flex-1">
              <Panel title={<><span className="text-fog">1</span>on the server</>} className="h-full">
                <div className="flex flex-col gap-0.5 px-3.5 pt-5 pb-3.5 md:gap-1 md:px-5 md:pt-6 md:pb-5">
                  <Line><Prompt /><code>curl -fsSL …/install.sh | sudo sh</code></Line>
                  <Line><span className="text-ok">✓</span><span className="text-mist">{brand.daemon} running · token printed</span></Line>
                </div>
              </Panel>
            </li>
            <li className="flex-1">
              <Panel title={<><span className="text-fog">2</span>on your laptop</>} className="h-full">
                <div className="flex flex-col gap-0.5 px-3.5 pt-5 pb-3.5 md:gap-1 md:px-5 md:pt-6 md:pb-5">
                  <Line><Prompt /><code>{brand.name} login http://5.75.12.4:7700 \</code></Line>
                  <Line><span> </span><code>    --token &lt;token&gt;</code></Line>
                  <Line><Prompt /><code>{brand.name}</code></Line>
                </div>
              </Panel>
            </li>
            <li className="flex-1">
              <Panel title={<><span className="text-fog">3</span>ship it</>} className="h-full">
                <div className="flex flex-col gap-0.5 px-3.5 pt-5 pb-3.5 md:gap-1 md:px-5 md:pt-6 md:pb-5">
                  <Line><Prompt /><code>{brand.name} service create acme-shop web \</code></Line>
                  <Line><span> </span><code>    --repo github.com/acme/web \</code></Line>
                  <Line><span> </span><code>    --port 3000 --domain shop.acme.dk</code></Line>
                  <Line><span className="text-ok">✓</span><span className="text-mist">healthy · https://shop.acme.dk</span></Line>
                </div>
              </Panel>
            </li>
          </ol>
        </section>

        <section aria-labelledby="details-title" className={`${container} flex flex-col gap-5 pt-[88px] pb-[88px] md:gap-10 md:pt-36 md:pb-[120px]`}>
          <SectionHead index="04" label="the details" id="details-title" title="What you are installing." />
          <dl className="flex flex-col border-t border-line md:pt-3">
            {specs.map(([term, detail]) => (
              <div key={term} className="flex justify-between gap-3 border-b border-line py-2.5 md:justify-start md:gap-6 md:border-none md:py-3 md:text-sm md:leading-[22px]">
                <dt className="md:w-40 md:shrink-0 md:text-mist">{term}</dt>
                <dd className="text-right text-mist md:text-left md:text-fog">{detail}</dd>
              </div>
            ))}
          </dl>
        </section>
      </main>

      <footer>
        <div aria-hidden="true" className="pixelated h-10 bg-[url(/dither/water.png)] bg-top bg-repeat-x md:h-14" />
        <div className={`${container} flex flex-col gap-3 border-t border-line pt-[18px] pb-7 md:flex-row md:items-center md:justify-between md:pt-5`}>
          <nav aria-label="Footer" className="flex gap-[22px] md:gap-6">
            <KeyLink keyName="g" href={links.github}>github</KeyLink>
            <KeyLink keyName="d" href={links.docs}>docs</KeyLink>
            <KeyLink keyName="i" href="#install">install</KeyLink>
          </nav>
          <p className="flex gap-2.5 text-mist">
            <span className="font-medium text-fog">{brand.name}</span>
            <span aria-hidden="true">·</span>
            <span>{brand.license} license</span>
            <span aria-hidden="true" className="hidden md:inline">·</span>
            <a href={links.github} className="hidden hover:text-fog md:inline">{brand.repo}</a>
          </p>
        </div>
      </footer>
    </>
  )
}
