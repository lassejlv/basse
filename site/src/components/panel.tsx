import type { ReactNode } from "react"
import { cn } from "@/lib/utils"

/** The TUI's panel: a thin rounded border with its title set into the top edge. */
export function Panel({
  title,
  aside,
  className,
  children,
}: {
  title: ReactNode
  aside?: ReactNode
  className?: string
  children: ReactNode
}) {
  return (
    <div className={cn("relative rounded-panel border border-line", className)}>
      <div className="absolute -top-2.5 left-2.5 flex gap-2 bg-quay px-1.5 whitespace-nowrap text-mist">{title}</div>
      {aside ? <div className="absolute -top-2.5 right-2.5 bg-quay px-1.5 whitespace-nowrap text-mist">{aside}</div> : null}
      {children}
    </div>
  )
}
