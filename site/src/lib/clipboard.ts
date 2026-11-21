import { Data, Effect } from "effect"

export class ClipboardError extends Data.TaggedError("ClipboardError")<{ readonly cause: unknown }> {}

export const copyText = (text: string) =>
  Effect.tryPromise({
    try: () => navigator.clipboard.writeText(text),
    catch: (cause) => new ClipboardError({ cause }),
  })
