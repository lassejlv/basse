import { useSyncExternalStore } from "react"

export const useMedia = (query: string) =>
  useSyncExternalStore(
    (onChange) => {
      const list = matchMedia(query)
      list.addEventListener("change", onChange)
      return () => list.removeEventListener("change", onChange)
    },
    () => matchMedia(query).matches,
    () => false,
  )
