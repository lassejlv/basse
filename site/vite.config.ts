import { readFileSync } from "node:fs"
import { fileURLToPath, URL } from "node:url"
import tailwindcss from "@tailwindcss/vite"
import { tanstackRouter } from "@tanstack/router-plugin/vite"
import react from "@vitejs/plugin-react"
import { defineConfig } from "vite"
import { brand } from "./src/brand.ts"

// The workspace version is the fallback release shown when the GitHub API is unreachable.
const cargo = readFileSync(new URL("../Cargo.toml", import.meta.url), "utf8")
const version = /^\[workspace\.package\][^[]*?^version\s*=\s*"([^"]+)"/m.exec(cargo)?.[1] ?? "0.0.0"

export default defineConfig({
  plugins: [
    tanstackRouter({ target: "react" }),
    react(),
    tailwindcss(),
    {
      name: "brand-html",
      transformIndexHtml: (html) =>
        html
          .replace("%TITLE%", `${brand.name} · ${brand.tagline.replace(/\.$/, "").toLowerCase()}`)
          .replace("%DESCRIPTION%", `${brand.tagline} ${brand.summary}`),
    },
  ],
  resolve: { alias: { "@": fileURLToPath(new URL("./src", import.meta.url)) } },
  define: { __BUILD_VERSION__: JSON.stringify(version) },
  worker: { format: "es" },
})
