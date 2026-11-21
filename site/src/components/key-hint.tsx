import type { ReactNode } from "react"
import { cn } from "@/lib/utils"

/** A key bar entry: the key in amber, the label muted, as a real link. */
export function KeyLink({
  keyName,
  href,
  children,
  className,
  external = false,
}: {
  keyName: string
  href: string
  children: ReactNode
  className?: string
  external?: boolean
}) {
  return (
    <a
      href={href}
      aria-keyshortcuts={keyName}
      {...(external ? { rel: "noopener" } : {})}
      className={cn("group inline-flex items-center gap-2 whitespace-nowrap rounded-[2px] text-mist hover:text-fog", className)}
    >
      <kbd className="font-mono text-amber">{keyName}</kbd>
      <span>{children}</span>
    </a>
  )
}
