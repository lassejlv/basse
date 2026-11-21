/**
 * Everything that names the product or points at it lives here, so a rename touches
 * one file (the site's counterpart of crates/common/src/brand.rs). vite.config.ts reads
 * it too, for the page title and description.
 */
export const brand = {
  /** Display name, and the name of the CLI binary. */
  name: "basse",
  /** The daemon binary. */
  daemon: "bassed",
  repo: "lassejlv/basse",
  license: "MIT",
  tagline: "Deploy apps to your own servers from the terminal.",
  summary: "A daemon on your server. A CLI and TUI on your laptop.",
} as const

export const links = {
  github: `https://github.com/${brand.repo}`,
  docs: `https://github.com/${brand.repo}#readme`,
  latestRelease: `https://api.github.com/repos/${brand.repo}/releases/latest`,
} as const

export const installCommand = `curl -fsSL https://raw.githubusercontent.com/${brand.repo}/main/install.sh | sudo sh`
