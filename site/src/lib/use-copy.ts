import { Effect, Fiber } from "effect"
import { useCallback, useEffect, useRef, useState } from "react"
import { copyText } from "./clipboard"

export type CopyState = "idle" | "copied" | "failed"

/** Copies text through the clipboard effect and shows the outcome for 1.6 s. */
export const useCopy = (text: string) => {
  const [state, setState] = useState<CopyState>("idle")
  const running = useRef<Fiber.Fiber<void> | null>(null)
  const stop = () => {
    if (running.current) Effect.runFork(Fiber.interrupt(running.current))
  }
  useEffect(() => stop, [])
  const copy = useCallback(() => {
    stop()
    running.current = Effect.runFork(
      copyText(text).pipe(
        Effect.as<CopyState>("copied"),
        Effect.catchTag("ClipboardError", () => Effect.succeed<CopyState>("failed")),
        Effect.tap((outcome) => Effect.sync(() => setState(outcome))),
        Effect.andThen(Effect.sleep("1600 millis")),
        Effect.andThen(Effect.sync(() => setState("idle"))),
      ),
    )
  }, [text])
  return { state, copy }
}
