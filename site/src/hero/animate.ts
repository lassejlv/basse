import { Effect } from "effect"
import type { ToWorker } from "./protocol"
import type { Scene } from "./scenes"

const post = (worker: Worker, message: ToWorker, transfer: Transferable[] = []) => worker.postMessage(message, transfer)

/**
 * Animates the water of a dithered scene on a canvas laid over its still, in a worker so
 * the main thread stays free. The worker only draws while the scene is on screen and the
 * tab is visible. Interrupting the effect removes the canvas and ends the worker.
 */
export const animateScene = (host: HTMLElement, scene: Scene) =>
  Effect.gen(function* () {
    if (!("transferControlToOffscreen" in HTMLCanvasElement.prototype)) return

    const canvas = yield* Effect.acquireRelease(
      Effect.sync(() => {
        const element = document.createElement("canvas")
        element.width = scene.meta.width
        element.height = scene.meta.height
        element.className = "pixelated absolute inset-0 size-full"
        element.setAttribute("aria-hidden", "true")
        host.append(element)
        return element
      }),
      (element) => Effect.sync(() => element.remove()),
    )
    const worker = yield* Effect.acquireRelease(
      Effect.sync(() => new Worker(new URL("./quay.worker.ts", import.meta.url), { type: "module" })),
      (worker) => Effect.sync(() => worker.terminate()),
    )
    const offscreen = canvas.transferControlToOffscreen()
    post(worker, { type: "start", canvas: offscreen, still: scene.still, source: scene.source, meta: scene.meta }, [offscreen])

    let onScreen = false
    const update = () => post(worker, { type: "run", running: onScreen && document.visibilityState === "visible" })

    yield* Effect.acquireRelease(
      Effect.sync(() => {
        const observer = new IntersectionObserver((entries) => {
          onScreen = entries.some((entry) => entry.isIntersecting)
          update()
        })
        observer.observe(host)
        return observer
      }),
      (observer) => Effect.sync(() => observer.disconnect()),
    )
    yield* Effect.acquireRelease(
      Effect.sync(() => document.addEventListener("visibilitychange", update)),
      () => Effect.sync(() => document.removeEventListener("visibilitychange", update)),
    )
    yield* Effect.never
  }).pipe(Effect.scoped)
