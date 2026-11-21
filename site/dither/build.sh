#!/usr/bin/env bash
# Regenerates every dithered asset the landing site uses, into site/public/dither/.
# Needs: dith >= 0.3.0 on PATH, python3 with pillow + numpy (PY=... picks one).
# The site animates the hero water itself (src/hero/water.ts), from the still, the source
# strip and the scene geometry (src/hero/<scene>.json) written here.
set -euo pipefail
cd "$(dirname "$0")"
PY=${PY:-python3}
FOG='#0E1113,#D8DEE3'
AMBER='#0E1113,#F5A524'
OUT=../public/dither
TMP=$(mktemp -d)
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$OUT"

# Two dith passes per scene: luminance in quay black + fog, the sodium light in quay
# black + amber; composite.py lays the amber pixels over the fog layer. A single
# three-colour pass sprinkles amber into the mid-greys.
scene() { # name width height
  $PY scene.py "$2" "$3" "$TMP/$1"
  dith apply "$TMP/$1-lum.png"   -o "$TMP/$1-lum-d.png"   --algorithm floyd-steinberg --colors "$FOG"   --json >/dev/null
  dith apply "$TMP/$1-amber.png" -o "$TMP/$1-amber-d.png" --algorithm floyd-steinberg --colors "$AMBER" --json >/dev/null
  $PY composite.py "$TMP/$1-lum-d.png" "$TMP/$1-amber-d.png" "$OUT/$1.png"
  $PY strip.py "$TMP/$1" "$OUT/$1-source.png"
  cp "$TMP/$1-meta.json" "../src/hero/$1.json"
}

scene hero-wide 1440 900
scene hero-narrow 1000 440

# Feature art at its two display sizes (a dither is never resized after the fact).
$PY icons.py "$TMP" 200 140
mkdir -p "$TMP/small" && $PY icons.py "$TMP/small" 100 70
for n in lighthouse tanks anchor; do
  dith apply "$TMP/$n.png"       -o "$OUT/$n.png"       --algorithm floyd-steinberg --colors "$FOG" --gamma 0.6 --json >/dev/null
  dith apply "$TMP/small/$n.png" -o "$OUT/$n-small.png" --algorithm floyd-steinberg --colors "$FOG" --gamma 0.6 --json >/dev/null
done

dith generate waves -o "$OUT/water.png" --canvas-width 1440 --canvas-height 56 \
  --stops '#0E1113,#4a5258,#0E1113' --frequency 1.4 --angle 0 --algorithm floyd-steinberg --colors "$FOG" --json >/dev/null
