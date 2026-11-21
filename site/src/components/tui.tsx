import type { ReactNode } from "react"
import { brand } from "@/config"
import { cn } from "@/lib/utils"

/*
 * The TUI's Overview screen as real text, after the Paper artboard "OVR-01 Overview" and the
 * ratatui layout in crates/tui. Sample data, not a screenshot, so it stays crisp and selectable.
 */

type Tone = "text" | "mist" | "amber" | "ok" | "info" | "err"
const tone: Record<Tone, string> = {
  text: "text-fog",
  mist: "text-mist",
  amber: "text-amber",
  ok: "text-ok",
  info: "text-info",
  err: "text-err",
}

const T = ({ c = "text", children, className }: { c?: Tone; children: ReactNode; className?: string }) => (
  <span className={cn(tone[c], className)}>{children}</span>
)

function Box({ title, aside, className, children }: { title: ReactNode; aside?: ReactNode; className?: string; children: ReactNode }) {
  return (
    <div className={cn("relative flex flex-col rounded-panel border border-line", className)}>
      <div className="absolute -top-2.5 left-2.5 flex bg-quay px-1.5 text-mist">{title}</div>
      {aside ? <div className="absolute -top-2.5 right-2.5 bg-quay px-1.5 text-mist">{aside}</div> : null}
      {children}
    </div>
  )
}

const services = [
  { dot: "●", dotTone: "ok", name: "web", source: "acme/shop@main", domain: "shop.acme.dk", deployed: "2m ago", depTone: "mist", cpu: "4%", mem: "212M" },
  { dot: "●", dotTone: "ok", name: "api", source: "acme/api@main", domain: "api.acme.dk", deployed: "2h ago", depTone: "mist", cpu: "11%", mem: "386M" },
  { dot: "⠹", dotTone: "info", name: "worker", source: "acme/api@main", domain: "—", deployed: "building", depTone: "info", cpu: "—", mem: "—" },
  { dot: "●", dotTone: "ok", name: "postgres", source: "postgres:17-alpine", domain: "—", deployed: "9d ago", depTone: "mist", cpu: "2%", mem: "148M", sourceTone: "mist" },
  { dot: "×", dotTone: "err", name: "imgproxy", source: "ghcr.io/imgproxy/imgproxy:v3", domain: "img.acme.dk", deployed: "failed", depTone: "err", cpu: "—", mem: "—", sourceTone: "mist" },
] as const

const activity = [
  ["14:02", "web", <span key="a">deployed 9f3c2a1 <T c="mist">41s</T></span>],
  ["13:48", "imgproxy", <T key="b" c="err">exited 137 · OOM</T>],
  ["13:47", "imgproxy", "deployed v3.24"],
  ["11:20", "api", <span key="c">deployed 77ab01e <T c="mist">1m12s</T></span>],
  ["09:05", "web", <span key="d">domain www.acme.dk <T c="mist">tls ok</T></span>],
  ["08:31", "postgres", <span key="e">volume pgdata <T c="mist">+2.1G</T></span>],
] as const

const keys = [["↵", "open"], ["d", "deploy"], ["r", "restart"], ["l", "logs"], ["n", "new"], ["/", "filter"], [":", "command"]] as const

const cpu = [3, 3, 4, 3, 2, 2, 3, 2, 2, 4, 5, 7, 10, 7, 4, 3, 3, 4, 3, 2, 2, 3, 2, 2, 2, 3, 4, 3, 2, 2, 3, 3, 2, 2, 3, 4, 6, 4, 3, 2, 2, 3, 2, 2, 3, 3, 2, 3]

function TopBar({ version, compact = false }: { version: string; compact?: boolean }) {
  return (
    <div className="flex h-5 shrink-0 items-center justify-between">
      <div className="flex items-center gap-2.5">
        <T c="amber" className="font-medium">{brand.name}</T>
        <T c="mist">›</T>
        <T>prod-eu-1</T>
        {compact ? null : <T c="mist">5.75.12.4 · {brand.daemon} {version}</T>}
        {compact ? null : <T c="ok">● connected</T>}
      </div>
      {compact ? (
        <T c="ok">● connected</T>
      ) : (
        <div className="flex items-center gap-[22px] text-mist">
          <span>cpu <T>23%</T> <T c="ok">▁▂▃▂▅▃▂</T></span>
          <span>mem <T>3.1/8.0G</T></span>
          <span>disk <T>41%</T></span>
          <span>proxy <T c="ok">●</T> <T>caddy</T></span>
        </div>
      )}
    </div>
  )
}

export function TuiOverview({ version }: { version: string }) {
  return (
    <div className="flex h-[800px] w-[1280px] flex-col gap-2 bg-quay px-3 py-2.5 text-[13px] leading-5">
      <TopBar version={version} />
      <div className="flex min-h-0 flex-1 gap-2.5">
        <div className="flex w-[248px] shrink-0 flex-col gap-3.5 pt-2.5">
          <Box title="Projects" className="flex-1 px-3 pt-3.5 pb-2.5">
            <div className="flex flex-col gap-0.5">
              {(
                [
                  ["▾", "acme-shop", "", "text", false, 0],
                  ["●", "production", "5", "amber", true, 1],
                  ["●", "staging", "4", "text", false, 1],
                  ["⠹", "pr-182", "2", "mist", false, 1],
                  ["", "", "", "text", false, -1],
                  ["▾", "internal-tools", "", "text", false, 0],
                  ["●", "production", "3", "text", false, 1],
                  ["", "", "", "text", false, -1],
                  ["▸", "blog", "1", "text", false, 0],
                  ["▸", "playground", "0", "mist", false, 0],
                ] as const
              ).map(([mark, name, count, nameTone, selected, depth], i) =>
                depth < 0 ? (
                  <div key={i} className="h-2" />
                ) : (
                  <div key={i} className={cn("flex h-5 items-center px-1.5", selected && "bg-hull")}>
                    <span style={{ width: depth * 16 }} className="shrink-0" />
                    <T c={depth ? (mark === "⠹" ? "info" : "ok") : "mist"} className="w-[18px] shrink-0">{mark}</T>
                    <T c={nameTone} className="flex-1">{name}</T>
                    <T c="mist">{count}</T>
                  </div>
                ),
              )}
            </div>
          </Box>
          <Box title="Servers" className="px-3 pt-3.5 pb-2">
            {(
              [
                ["●", "ok", "prod-eu-1", "local"],
                ["●", "ok", "edge-fra", "ssh"],
                ["○", "mist", "home-lab", "offline"],
              ] as const
            ).map(([mark, markTone, name, kind]) => (
              <div key={name} className="flex h-[22px] items-center px-1.5">
                <T c={markTone} className="w-[18px]">{mark}</T>
                <T className="flex-1">{name}</T>
                <T c="mist">{kind}</T>
              </div>
            ))}
          </Box>
        </div>
        <div className="flex min-w-0 flex-1 flex-col gap-3.5 pt-2.5">
          <Box
            title={<><T c="mist">acme-shop</T>&nbsp;<T c="mist">/</T>&nbsp;<T>production</T>&nbsp;<T c="mist">· 5 services</T></>}
            aside="filter: all"
            className="h-[168px] px-3 pt-3.5"
          >
            <div className="flex h-6 items-center px-1.5 text-mist">
              <span className="w-[22px]" />
              <span className="w-[120px]">NAME</span>
              <span className="w-[270px]">SOURCE</span>
              <span className="w-[180px]">DOMAIN</span>
              <span className="w-24">DEPLOYED</span>
              <span className="w-14">CPU</span>
              <span>MEM</span>
            </div>
            {services.map((s, i) => (
              <div key={s.name} className={cn("flex h-6 items-center px-1.5", i < 2 && "bg-hull")}>
                <T c={s.dotTone} className="w-[22px]">{s.dot}</T>
                <T c={i === 0 ? "amber" : "text"} className="w-[120px]">{s.name}</T>
                <T c={"sourceTone" in s ? s.sourceTone : "text"} className="w-[270px]">{s.source}</T>
                <T c={s.domain === "—" ? "mist" : "text"} className="w-[180px]">{s.domain}</T>
                <T c={s.depTone} className="w-24">{s.deployed}</T>
                <T c={s.cpu === "—" ? "mist" : "text"} className="w-14">{s.cpu}</T>
                <T c={s.mem === "—" ? "mist" : "text"}>{s.mem}</T>
              </div>
            ))}
          </Box>
          <div className="flex min-h-0 flex-1 gap-2.5">
            <Box title="web" className="w-[548px] shrink-0 px-[18px] pt-3.5 pb-3.5">
              <div className="flex flex-col">
                {(
                  [
                    ["image", <>built from 9f3c2a1 <T c="mist">&nbsp;fix: cart rounding</T></>],
                    ["route", <>https://shop.acme.dk <T c="mist">&nbsp;→ :3000</T></>],
                    ["", <>https://www.acme.dk <T c="mist">&nbsp;→ redirect</T></>],
                    ["health", <><T c="ok">✓ /healthz</T> <T c="mist">&nbsp;200 in 18ms</T></>],
                    ["volumes", <>uploads <T c="mist">&nbsp;/app/uploads&nbsp; 1.2G</T></>],
                    ["env", <>14 vars <T c="mist">&nbsp;3 secret</T></>],
                    ["restart", <>unless-stopped <T c="mist">&nbsp;0 restarts</T></>],
                  ] as const
                ).map(([label, value], i) => (
                  <div key={i} className="flex h-[26px] items-center">
                    <T c="mist" className="w-24 shrink-0">{label}</T>
                    <T>{value}</T>
                  </div>
                ))}
              </div>
              <div className="mt-auto flex flex-col gap-2.5 border-t border-line pt-3">
                <div className="flex justify-between">
                  <T c="mist">cpu · last hour</T>
                  <span><T>4%</T> <T c="mist">peak</T> <T c="mist">31%</T></span>
                </div>
                <div className="flex h-3 items-end">
                  {cpu.map((h, i) => (
                    <span key={i} className="w-2.5 bg-ok" style={{ height: h + 1 }} />
                  ))}
                </div>
                <div className="flex justify-between pt-1">
                  <T c="mist">memory</T>
                  <span><T>212M</T> <T c="mist">/ 512M</T></span>
                </div>
                <div className="h-2.5 w-full bg-info" />
              </div>
            </Box>
            <Box title="Activity" className="flex-1 px-[18px] pt-3.5">
              {activity.map(([time, name, what], i) => (
                <div key={i} className="flex h-[26px] items-center">
                  <T c="mist" className="w-[60px] shrink-0">{time}</T>
                  <T className="w-[88px] shrink-0">{name}</T>
                  <T>{what}</T>
                </div>
              ))}
            </Box>
          </div>
        </div>
      </div>
      <div className="flex h-5 shrink-0 items-center justify-between">
        <div className="flex gap-[22px]">
          {[["↑↓", "move"] as const, ...keys].map(([k, label]) => (
            <span key={label} className="flex gap-2"><T c="amber">{k}</T><T c="mist">{label}</T></span>
          ))}
        </div>
        <div className="flex gap-[22px]">
          {([[",", "settings"], ["?", "help"], ["q", "quit"]] as const).map(([k, label]) => (
            <span key={label} className="flex gap-2"><T c="amber">{k}</T><T c="mist">{label}</T></span>
          ))}
        </div>
      </div>
    </div>
  )
}

/** The same screen cut down to what fits a phone: the service list, recent activity and the key bar. */
export function TuiCompact({ version }: { version: string }) {
  return (
    <div className="flex flex-col gap-4 bg-quay px-2.5 pt-3 pb-2.5 text-xs leading-[19px]">
      <TopBar version={version} compact />
      <Box title={<><T c="mist">acme-shop /&nbsp;</T><T>production</T></>} className="px-1.5 pt-3 pb-1.5">
        {services.map((s, i) => (
          <div key={s.name} className={cn("flex h-5 items-center px-1", i === 0 && "bg-hull")}>
            <T c={s.dotTone} className="w-4 shrink-0">{s.dot}</T>
            <T c={i === 0 ? "amber" : "text"} className="w-[84px] shrink-0">{s.name}</T>
            <T className="flex-1 truncate">{s.domain}</T>
            <T c="mist">{s.deployed.replace(" ago", "")}</T>
          </div>
        ))}
      </Box>
      <Box title="Activity" className="px-2.5 pt-3 pb-1.5">
        {(
          [
            ["14:02", "web", "deployed 9f3c2a1"],
            ["13:48", "imgproxy", <T key="err" c="err">exited 137 · OOM</T>],
            ["11:20", "api", "deployed 77ab01e"],
          ] as const
        ).map(([time, name, what]) => (
          <div key={time} className="flex h-5 items-center gap-2.5 whitespace-nowrap">
            <T c="mist">{time}</T>
            <T className="w-[58px] shrink-0">{name}</T>
            <T>{what}</T>
          </div>
        ))}
      </Box>
      <div className="flex gap-3.5">
        {([["↵", "open"], ["d", "deploy"], ["l", "logs"], ["?", "help"]] as const).map(([k, label]) => (
          <span key={label} className="flex gap-1.5"><T c="amber">{k}</T><T c="mist">{label}</T></span>
        ))}
      </div>
    </div>
  )
}
