import { useEffect, useEffectEvent } from "react"

/**
 * Single-key shortcuts like the TUI's key bar. Ignored while typing in a field or when a
 * modifier is held, so browser shortcuts keep working.
 */
export const useShortcuts = (bindings: Record<string, () => void>) => {
  const onKey = useEffectEvent((event: KeyboardEvent) => {
    if (event.defaultPrevented || event.metaKey || event.ctrlKey || event.altKey || event.repeat) return
    const target = event.target as HTMLElement | null
    if (target?.closest("input, textarea, select, [contenteditable]")) return
    const action = bindings[event.key]
    if (!action) return
    event.preventDefault()
    action()
  })
  useEffect(() => {
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [])
}
