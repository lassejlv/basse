import type { SceneMeta } from "./water"

export type ToWorker =
  | {
      readonly type: "start"
      readonly canvas: OffscreenCanvas
      readonly still: string
      readonly source: string
      readonly meta: SceneMeta
    }
  | { readonly type: "run"; readonly running: boolean }

/** Frames per second the water is drawn at, and the seconds one full loop of its motion takes. */
export const FPS = 30
export const LOOP_SECONDS = 6
