import { Effect, Fiber } from "effect"
import { type DependencyList, useEffect } from "react"

/** Runs an effect for as long as the component is mounted; unmounting interrupts it and runs its finalizers. */
export const useEffectFiber = (effect: () => Effect.Effect<unknown, never>, deps: DependencyList) => {
  useEffect(() => {
    const fiber = Effect.runFork(effect())
    return () => {
      Effect.runFork(Fiber.interrupt(fiber))
    }
    // oxlint-disable-next-line react-hooks/exhaustive-deps -- the caller's list; call sites are checked through additionalHooks
  }, deps)
}
