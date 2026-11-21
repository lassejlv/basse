import { Effect } from "effect"
import { useState } from "react"
import { Landing } from "@/components/landing"
import { buildVersion } from "@/config"
import { latestVersion } from "@/lib/release"
import { useEffectFiber } from "@/lib/use-effect"

export function Home() {
  const [version, setVersion] = useState(buildVersion)
  useEffectFiber(() => latestVersion.pipe(Effect.tap((v) => Effect.sync(() => setVersion(v)))), [])
  return <Landing version={version} />
}
