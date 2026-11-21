import { Data, Effect, Latch, Schedule } from "effect"
import { FPS, LOOP_SECONDS, type ToWorker } from "./protocol"
import { makeWater } from "./water"

class AssetError extends Data.TaggedError("AssetError")<{ readonly url: string; readonly cause: unknown }> {}
class CanvasError extends Data.TaggedError("CanvasError")<{}> {}

const bitmap = (url: string) =>
  Effect.tryPromise({
    try: async (signal) => createImageBitmap(await (await fetch(url, { signal })).blob()),
    catch: (cause) => new AssetError({ url, cause }),
  })

const context = (canvas: OffscreenCanvas) => {
  const ctx = canvas.getContext("2d", { alpha: false })
  return ctx ? Effect.succeed(ctx) : Effect.fail(new CanvasError())
}

/** Opened while the hero is on screen and the tab is visible; the loop waits on it otherwise. */
const visible = Latch.makeUnsafe(false)

const animate = (start: Extract<ToWorker, { type: "start" }>) =>
  Effect.gen(function* () {
    const { canvas, meta } = start
    const ctx = yield* context(canvas)
    const [still, source] = yield* Effect.all([bitmap(start.still), bitmap(start.source)], { concurrency: 2 })
    ctx.drawImage(still, 0, 0)
    still.close()

    const reader = yield* context(new OffscreenCanvas(source.width, source.height))
    reader.drawImage(source, 0, 0)
    const water = makeWater(meta, reader.getImageData(0, 0, source.width, source.height).data)
    source.close()

    const frame = new ImageData(meta.width, water.rows)
    let elapsed = 0
    let last = performance.now()
    const draw = Effect.sync(() => {
      const now = performance.now()
      elapsed += Math.min(now - last, 1000 / FPS)
      last = now
      water.render((elapsed / (LOOP_SECONDS * 1000)) % 1, frame.data)
      ctx.putImageData(frame, 0, meta.waterTop)
    })
    yield* visible.await.pipe(Effect.andThen(draw), Effect.repeat(Schedule.spaced(`${Math.round(1000 / FPS)} millis`)))
  }).pipe(Effect.catchTags({ AssetError: Effect.logWarning, CanvasError: Effect.logWarning }))

self.onmessage = (event: MessageEvent<ToWorker>) => {
  const message = event.data
  if (message.type === "start") Effect.runFork(animate(message))
  else if (message.running) visible.openUnsafe()
  else visible.closeUnsafe()
}
