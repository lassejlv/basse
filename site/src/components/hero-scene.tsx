import { Effect } from "effect"
import { useRef } from "react"
import { animateScene } from "@/hero/animate"
import { scenes } from "@/hero/scenes"
import { useEffectFiber } from "@/lib/use-effect"
import { useMedia } from "@/lib/use-media"

/**
 * The dithered night quay behind the hero copy: the dith still, with the water animated
 * on a canvas on top. Reduced motion keeps the still. From 768 px up the wide scene sits
 * at the top right; below it the narrow scene sits at the bottom with the lamp and the
 * crane in view. The layout is CSS only, so the prerendered page is right before any
 * script runs.
 */
export function HeroScene() {
  const host = useRef<HTMLDivElement>(null)
  const wide = useMedia("(min-width: 768px)")
  const still = useMedia("(prefers-reduced-motion: reduce)")

  useEffectFiber(
    () => (host.current && !still ? animateScene(host.current, wide ? scenes.wide : scenes.narrow) : Effect.void),
    [wide, still],
  )

  return (
    <div aria-hidden="true" className="pointer-events-none absolute inset-0 overflow-hidden">
      <div
        ref={host}
        className="absolute bottom-0 left-[min(0px,calc(100%-910px))] h-[440px] w-[1000px] md:top-0 md:right-[max(0px,calc((100%-1440px)/2))] md:bottom-auto md:left-auto md:h-[900px] md:w-[1440px]"
      >
        <picture>
          <source media="(min-width: 768px)" srcSet={scenes.wide.still} width={1440} height={900} />
          <img src={scenes.narrow.still} alt="" width={1000} height={440} fetchPriority="high" className="pixelated size-full" />
        </picture>
      </div>
    </div>
  )
}
