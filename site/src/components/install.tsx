import { Fragment } from "react"
import { Button } from "@/components/ui/button"
import { Panel } from "@/components/panel"
import { installCommand } from "@/config"
import type { CopyState } from "@/lib/use-copy"

const label: Record<CopyState, string> = { idle: "copy", copied: "copied", failed: "copy failed" }

/** Break opportunities after each slash and before the pipe, for narrow screens. */
const breakable = (command: string) =>
  command.split(/(?<=\/)|(?=\| )/).map((part, i) => (
    <Fragment key={i}>
      {i > 0 ? <wbr /> : null}
      {part}
    </Fragment>
  ))

export function InstallCommand({ state, onCopy }: { state: CopyState; onCopy: () => void }) {
  return (
    <Panel title="install on a fresh Linux server" className="bg-quay">
      <div className="flex flex-col gap-4 px-4 pt-5 pb-4 md:flex-row md:items-center md:justify-between md:gap-6 md:pt-[18px] md:pr-4 md:pb-3.5 md:pl-5">
        <p className="flex gap-2.5 md:gap-3.5 md:text-[15px] md:leading-6">
          <span aria-hidden="true" className="text-mist select-none">$</span>
          <code className="min-w-0 break-normal md:whitespace-nowrap">{breakable(installCommand)}</code>
        </p>
        <Button size="sm" onClick={onCopy} aria-keyshortcuts="c" className="self-end md:self-auto">
          <kbd className="font-medium">c</kbd>
          <span aria-live="polite">{label[state]}</span>
        </Button>
      </div>
    </Panel>
  )
}
