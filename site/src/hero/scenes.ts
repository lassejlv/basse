import narrow from "./hero-narrow.json"
import wide from "./hero-wide.json"
import type { SceneMeta } from "./water"

export interface Scene {
  readonly still: string
  readonly source: string
  readonly meta: SceneMeta
}

/** Rendered by site/dither/build.sh. The wide scene backs viewports from 768 px up. */
export const scenes = {
  wide: { still: "/dither/hero-wide.png", source: "/dither/hero-wide-source.png", meta: wide },
  narrow: { still: "/dither/hero-narrow.png", source: "/dither/hero-narrow-source.png", meta: narrow },
} satisfies Record<string, Scene>

export type SceneName = keyof typeof scenes
