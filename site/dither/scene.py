"""Source art for the Basse hero background: a container quay at night under one sodium lamp.

The scene is composed as a full-bleed background: the upper left is dark, quiet sky
for the hero copy; the quay, the ship-to-shore crane and the lamp sit low and right.

Writes greyscale sources that dith then dithers:
  <out>-lum.png    scene luminance (dithered to quay black + fog)
  <out>-amber.png  sodium light intensity (dithered to quay black + amber)
  <out>-meta.json  geometry the site's animated water needs (size, unit, water line, lamp x)

usage: scene.py WIDTH HEIGHT OUT_PREFIX [PHASE 0..1]
"""
import math
import sys

import numpy as np
from PIL import Image, ImageDraw

W, H = int(sys.argv[1]), int(sys.argv[2])
OUT = sys.argv[3]
PHASE = float(sys.argv[4]) if len(sys.argv) > 4 else 0.0
TAU = 2 * math.pi

yy, xx = np.mgrid[0:H, 0:W].astype(np.float64)
u, v = xx / W, yy / H
U = H / 820.0  # vertical unit: sizes below were tuned for an 820 px tall canvas


def vnoise(scale, seed):
    r = np.random.default_rng(seed)
    gw, gh = int(W / scale) + 3, int(H / scale) + 3
    g = r.random((gh, gw))
    xs, ys = xx / scale, yy / scale
    x0, y0 = xs.astype(int), ys.astype(int)
    fx, fy = xs - x0, ys - y0
    fx, fy = fx * fx * (3 - 2 * fx), fy * fy * (3 - 2 * fy)
    a, b = g[y0, x0], g[y0, x0 + 1]
    c, d = g[y0 + 1, x0], g[y0 + 1, x0 + 1]
    return a * (1 - fx) * (1 - fy) + b * fx * (1 - fy) + c * (1 - fx) * fy + d * fx * fy


def mask(draw_fn):
    m = Image.new("L", (W, H), 0)
    draw_fn(ImageDraw.Draw(m))
    return np.asarray(m, dtype=np.float64) / 255.0


def px(f):
    return f * W


def py(f):
    return f * H


def smooth(a, b, x):
    t = np.clip((x - a) / (b - a), 0, 1)
    return t * t * (3 - 2 * t)


QUAY = 0.80
CH = 46 * U / H  # container height as a fraction of H
CW40, CW20 = 150 * U / W, 76 * U / W  # container widths as fractions of W

# --- sky: dark zenith, sodium-lit fog low and to the right ---------------------------
fog = 0.6 * vnoise(W / 7, 1) + 0.4 * vnoise(W / 18, 2)
t = np.clip(v / QUAY, 0, 1)
sky = 0.34 * t ** 2.2 + (fog - 0.5) * 0.14 * t
sky *= 0.25 + 0.75 * smooth(0.30, 0.80, u)
L = 0.02 + sky
A = np.zeros((H, W))

ridge = QUAY - 0.06 + 0.014 * np.sin(u * 5.0 + 1.3) + 0.008 * np.sin(u * 13.0)
L = np.where((v > ridge) & (v < QUAY), 0.10 + 0.03 * fog, L)
rng = np.random.default_rng(11)
for _ in range(int(W / 40)):
    lx = rng.uniform(0.02, 0.98) * W
    ly = (QUAY - 0.035 + rng.uniform(-0.008, 0.012)) * H
    L = np.where(((xx - lx) ** 2 + (yy - ly) ** 2) < (2.6 * U) ** 2, 0.7, L)


def stack(x0, x1, base, rows, seed):
    global L
    r = np.random.default_rng(seed)
    x = x0
    rib = max(2.0, 5.5 * U)
    while x < x1 - 0.01:
        w = CW40 if r.random() < 0.6 else CW20
        w = min(w, x1 - x)
        n = max(1, rows if r.random() < 0.7 else rows - 1)
        for i in range(n):
            top, bot = base - (i + 1) * CH, base - i * CH
            tone = r.uniform(0.12, 0.42)
            m = mask(lambda d: d.rectangle([px(x) + 1, py(top) + 1, px(x + w) - 2, py(bot) - 2], fill=255))
            ribs = 0.06 * (np.sin(xx / rib * math.pi) > 0.3)
            shade = tone + ribs - 0.08 * ((yy - py(top)) / (py(bot) - py(top)))
            L = np.where(m > 0, shade, L)
            if r.random() < 0.5:
                e = mask(lambda d: d.rectangle([px(x + w) - 14 * U, py(top) + 2, px(x + w) - 3, py(bot) - 3], fill=255))
                L = np.where(e > 0, tone * 0.55, L)
            lit = mask(lambda d: d.line([px(x) + 1, py(top) + 1, px(x + w) - 2, py(top) + 1], fill=255, width=max(1, int(3 * U))))
            L = np.where(lit > 0, np.minimum(1.0, tone + 0.35), L)
        x += w + 5 / W


# --- ship with a deck load, right ------------------------------------------------
DECK = QUAY - 0.085
hull = mask(lambda d: d.polygon([(px(0.66), py(DECK)), (px(1.02), py(DECK)), (px(1.02), py(QUAY + 0.005)), (px(0.675), py(QUAY + 0.005))], fill=255))
L = np.where(hull > 0, 0.05, L)
hl = mask(lambda d: d.line([px(0.664), py(DECK + 0.014), px(1.02), py(DECK + 0.014)], fill=255, width=max(1, int(2 * U))))
L = np.where(hl > 0, 0.35, L)
stack(0.675, 1.02, DECK, 3, 21)

# --- ship-to-shore crane --------------------------------------------------------
lw = max(2, int(4 * U))
BOOM = QUAY - 0.53


def crane(d):
    for lx in (0.775, 0.885):
        d.line([px(lx), py(QUAY), px(lx), py(BOOM)], fill=255, width=lw)
    d.line([px(0.775), py(BOOM + 0.15), px(0.885), py(BOOM + 0.15)], fill=255, width=lw)
    d.line([px(0.775), py(BOOM + 0.15), px(0.885), py(BOOM)], fill=255, width=max(1, lw // 2))
    d.line([px(0.665), py(BOOM), px(1.02), py(BOOM)], fill=255, width=lw)
    d.line([px(0.665), py(BOOM + 0.035), px(1.02), py(BOOM + 0.035)], fill=255, width=max(1, lw // 2))
    s = 0.665
    step = 26 * U / W
    while s < 1.0:
        d.line([px(s), py(BOOM + 0.035), px(s + step), py(BOOM)], fill=255, width=1)
        d.line([px(s + step), py(BOOM), px(s + 2 * step), py(BOOM + 0.035)], fill=255, width=1)
        s += 2 * step
    d.line([px(0.83), py(BOOM), px(0.83), py(BOOM - 0.10)], fill=255, width=lw)
    d.line([px(0.83), py(BOOM - 0.10), px(0.69), py(BOOM)], fill=255, width=2)
    d.line([px(0.83), py(BOOM - 0.10), px(0.99), py(BOOM)], fill=255, width=2)
    d.rectangle([px(0.700), py(BOOM + 0.035), px(0.722), py(BOOM + 0.055)], fill=255)
    d.line([px(0.711), py(BOOM + 0.055), px(0.711), py(BOOM + 0.21)], fill=255, width=1)


L = np.where(mask(crane) > 0, 0.62, L)
bx0, bx1 = 0.711 - CW40 / 2, 0.711 + CW40 / 2
box = mask(lambda d: d.rectangle([px(bx0), py(BOOM + 0.21), px(bx1), py(BOOM + 0.21 + CH)], fill=255))
L = np.where(box > 0, 0.30 + 0.05 * (np.sin(xx / max(2.0, 5.5 * U) * math.pi) > 0.3), L)
bt = mask(lambda d: d.line([px(bx0), py(BOOM + 0.211), px(bx1), py(BOOM + 0.211)], fill=255, width=max(1, int(3 * U))))
L = np.where(bt > 0, 0.75, L)

# --- quay with container stacks ---------------------------------------------------
QTOP = QUAY - 0.012
stack(0.30, 0.545, QTOP, 3, 9)
stack(0.0, 0.17, QTOP, 1, 5)
deck = mask(lambda d: d.rectangle([0, py(QTOP), px(0.665), py(QUAY + 0.012)], fill=255))
L = np.where(deck > 0, 0.20, L)
edge = mask(lambda d: d.line([0, py(QUAY + 0.012), px(0.665), py(QUAY + 0.012)], fill=255, width=max(1, int(3 * U))))
L = np.where(edge > 0, 0.85 * (0.5 + 0.5 * smooth(0.05, 0.4, u)), L)
for bxx in (0.22, 0.26, 0.60):
    b = mask(lambda d: d.rectangle([px(bxx), py(QTOP) - 12 * U, px(bxx) + 11 * U, py(QTOP)], fill=255))
    L = np.where(b > 0, 0.06, L)

# --- the sodium lamp ------------------------------------------------------------------
LX, LY = 0.585, QUAY - 0.17
pw = max(2, lw - 1)
L = np.where(mask(lambda d: d.line([px(LX), py(QTOP), px(LX), py(LY)], fill=255, width=pw)) > 0, 0.55, L)
L = np.where(mask(lambda d: d.line([px(LX), py(LY), px(LX - 0.028), py(LY)], fill=255, width=pw)) > 0, 0.55, L)
head = mask(lambda d: d.rectangle([px(LX - 0.040), py(LY) + 1, px(LX - 0.018), py(LY) + 11 * U], fill=255))
A = np.maximum(A, head)
cx = (LX - 0.029) * W
top_y = py(LY) + 11 * U
spread = np.clip((yy - top_y) / (py(QUAY) - top_y), 0, 1)
inside = (yy > top_y) & (yy < py(QUAY + 0.012))
cone = np.clip(1.0 - np.abs(xx - cx) / (8 * U + 90 * U * spread), 0, 1)
A = np.maximum(A, np.where(inside, cone ** 1.6, 0) * (0.30 + 0.22 * spread))

# --- water: broken reflections that fade into the page ------------------------------
wtop = int(py(QUAY + 0.014))
for y in range(wtop, H):
    k = y - wtop
    depth = k / max(1, H - wtop)
    src_y = max(0, int(wtop - k * 0.9) - 2)
    band = math.sin(k / U * (0.45 - 0.25 * depth) + PHASE * TAU * 3)
    shift = int(6 * U * (1 + depth * 4) * math.sin(k / U * 0.21 + PHASE * TAU * 2))
    keep = 0.75 if band > -0.2 else 0.15
    fade = (1 - depth) ** 1.1
    L[y] = 0.02 + (np.roll(L[src_y], shift) - 0.02) * keep * fade
    A[y] = np.roll(A[src_y], shift) * (0.9 if band > 0.1 else 0.0) * fade

for y in range(wtop, H):
    k = y - wtop
    depth = k / max(1, H - wtop)
    if math.sin(k / U * 0.9 + PHASE * TAU * 4) < 0.25:
        continue
    half = (9 + 45 * depth) * U * (0.6 + 0.4 * math.sin(k * 0.37 + PHASE * TAU))
    x0 = int(cx - half + 4 * U * math.sin(k * 0.13 + PHASE * TAU * 2))
    x1 = int(cx + half)
    A[y, max(0, x0):max(0, x1)] = np.maximum(A[y, max(0, x0):max(0, x1)], 0.75 * (1 - depth) ** 1.2)

import json

with open(f"{OUT}-meta.json", "w") as fh:
    json.dump({"width": W, "height": H, "unit": U, "waterTop": wtop, "lampX": cx}, fh)
Image.fromarray((np.clip(L, 0, 1) * 255).astype(np.uint8), "L").save(f"{OUT}-lum.png")
Image.fromarray((np.clip(A, 0, 1) * 255).astype(np.uint8), "L").save(f"{OUT}-amber.png")
