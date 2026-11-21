import { Data, Effect } from "effect"
import { buildVersion, links } from "@/config"

export class ReleaseError extends Data.TaggedError("ReleaseError")<{ readonly cause: unknown }> {}

/** The one field we read from GitHub's release object. A hand check rather than Schema, which would add about 20 kB to the page. */
const tagName = (body: unknown) =>
  typeof body === "object" && body !== null && "tag_name" in body && typeof body.tag_name === "string"
    ? Effect.succeed(body.tag_name)
    : Effect.fail(new ReleaseError({ cause: "response has no tag_name" }))
const CACHE_KEY = "release:latest"
const CACHE_TTL_MS = 60 * 60 * 1000

const readCache = Effect.sync(() => {
  const raw = sessionStorage.getItem(CACHE_KEY)
  if (!raw) return undefined
  const [at, version] = raw.split(" ")
  return version && Date.now() - Number(at) < CACHE_TTL_MS ? version : undefined
})

const writeCache = (version: string) =>
  Effect.sync(() => sessionStorage.setItem(CACHE_KEY, `${Date.now()} ${version}`))

/** The newest release tag from GitHub, without the leading "v". */
const fetchLatest = Effect.tryPromise({
  try: async (signal) => {
    const response = await fetch(links.latestRelease, {
      signal,
      headers: { Accept: "application/vnd.github+json" },
    })
    if (!response.ok) throw new Error(`GitHub answered ${response.status}`)
    return (await response.json()) as unknown
  },
  catch: (cause) => new ReleaseError({ cause }),
}).pipe(
  Effect.flatMap(tagName),
  Effect.map((tag) => tag.replace(/^v/, "")),
  Effect.tap(writeCache),
)

/**
 * The version to show: cached for an hour in the session, then from GitHub with a
 * 4 second budget, and the build-time version when either fails.
 */
export const latestVersion: Effect.Effect<string> = readCache.pipe(
  Effect.flatMap((cached) => (cached ? Effect.succeed(cached) : fetchLatest)),
  Effect.timeout("4 seconds"),
  Effect.orElseSucceed(() => buildVersion),
)
