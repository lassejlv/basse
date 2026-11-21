# Landing site

Vite, React and TypeScript (strict), Tailwind CSS v4, shadcn/ui as the component base, TanStack Router (file routes in `src/routes`) and Effect for our own logic. Bun only.

```sh
bun install
bun run dev         # http://localhost:5173
bun run typecheck
bun run lint
bun run build       # dist/, static files
```

## Where things live

- `src/brand.ts`: the product name, repo, tagline and install command. Nothing else names the product.
- `src/index.css`: the colour tokens (the same harbor-at-night palette as the TUI and the Paper file "Basse TUI", page `11 Landing`) and the mapping of shadcn's slots onto them.
- `src/lib/release.ts`: the latest release from the GitHub API, cached for an hour, with the workspace version from `Cargo.toml` as the fallback.
- `src/hero/`: the animated hero. `water.ts` rebuilds and dithers the water each frame (Floyd–Steinberg, exact palette) in `quay.worker.ts` on an OffscreenCanvas at 30 fps. It pauses off screen and in hidden tabs, and `prefers-reduced-motion` keeps the still.
- `dither/`: the scripts that render every dithered image with [dith](https://github.com/lassejlv/dith). Run `dither/build.sh` (needs dith and python3 with pillow and numpy) after changing `scene.py` or `icons.py`; it writes `public/dither/` and `src/hero/*.json`.

Key hints work as shortcuts: `c` copies the install command, `i` jumps to it, `g` opens GitHub, `d` the docs, `f` and `h` jump to the sections.
