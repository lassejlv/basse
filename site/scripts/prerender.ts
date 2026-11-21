import { readFileSync, rmSync, writeFileSync } from "node:fs"

// Built by `vite build --ssr src/prerender.tsx --outDir dist-ssr`; see the build script in package.json.
const { render } = (await import(new URL("../dist-ssr/prerender.js", import.meta.url).href)) as { render: () => Promise<string> }

const page = new URL("../dist/index.html", import.meta.url)
const html = readFileSync(page, "utf8")
if (!html.includes('<div id="root"></div>')) throw new Error("dist/index.html has no empty #root to fill")
writeFileSync(page, html.replace('<div id="root"></div>', `<div id="root">${await render()}</div>`))
rmSync(new URL("../dist-ssr", import.meta.url), { recursive: true, force: true })
