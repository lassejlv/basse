"""Source art for the three small feature illustrations, as shaded greyscale.

usage: icons.py OUT_DIR SIZE_W SIZE_H
Writes lighthouse.png (proxy), tanks.png (databases), anchor.png (backups).
"""
import math
import sys

import numpy as np
from PIL import Image, ImageDraw

OUT, W, H = sys.argv[1], int(sys.argv[2]), int(sys.argv[3])
yy, xx = np.mgrid[0:H, 0:W].astype(np.float64)
BG = 0.0


def save(arr, name):
    Image.fromarray((np.clip(arr, 0, 1) * 255).astype(np.uint8), "L").save(f"{OUT}/{name}.png")


def mask(fn):
    m = Image.new("L", (W, H), 0)
    fn(ImageDraw.Draw(m))
    return np.asarray(m, dtype=np.float64) / 255.0


# --- lighthouse: the proxy that points traffic home ---------------------------
L = np.full((H, W), BG)
cx = W * 0.30
base_y, top_y = H * 0.92, H * 0.30
tower = mask(lambda d: d.polygon([(cx - W * 0.07, base_y), (cx + W * 0.07, base_y), (cx + W * 0.045, top_y), (cx - W * 0.045, top_y)], fill=255))
shade = 0.75 - 0.55 * np.clip((xx - (cx - W * 0.07)) / (W * 0.14), 0, 1)
L = np.where(tower > 0, shade, L)
for k in range(3):
    by = top_y + (base_y - top_y) * (0.22 + 0.27 * k)
    band = mask(lambda d: d.rectangle([0, by, W, by + H * 0.045], fill=255))
    L = np.where((tower > 0) & (band > 0), shade * 0.25, L)
lamp = mask(lambda d: d.rectangle([cx - W * 0.035, top_y - H * 0.11, cx + W * 0.035, top_y], fill=255))
L = np.where(lamp > 0, 1.0, L)
roof = mask(lambda d: d.polygon([(cx - W * 0.05, top_y - H * 0.11), (cx + W * 0.05, top_y - H * 0.11), (cx, top_y - H * 0.19)], fill=255))
L = np.where(roof > 0, 0.45, L)
# the beam: a wedge to the right that fades with distance
ly = top_y - H * 0.055
ang = np.arctan2(yy - ly, xx - cx)
dist = np.hypot(xx - cx, yy - ly)
wedge = (np.abs(ang - 0.05) < 0.16) & (xx > cx + W * 0.04)
beam = np.where(wedge, 0.55 * np.clip(1 - dist / (W * 0.75), 0, 1) * (1 - np.abs(ang - 0.05) / 0.16), 0)
L = np.maximum(L, beam)
ground = mask(lambda d: d.rectangle([0, base_y, W, H], fill=255))
L = np.where(ground > 0, 0.12, L)
save(L, "lighthouse")

# --- tanks: three stacked storage cylinders (databases) -------------------------
L = np.full((H, W), BG)
cw, ch = W * 0.42, H * 0.20
x0 = W * 0.5 - cw / 2
for i in range(3):
    top = H * 0.18 + i * ch * 1.05
    body = mask(lambda d: d.rectangle([x0, top + ch * 0.2, x0 + cw, top + ch], fill=255))
    u = (xx - x0) / cw
    cyl = 0.15 + 0.7 * np.clip(np.sin(np.clip(u, 0, 1) * math.pi) ** 0.8 * (1.1 - u * 0.6), 0, 1)
    L = np.where(body > 0, cyl, L)
    bottom = mask(lambda d: d.ellipse([x0, top + ch * 0.8, x0 + cw, top + ch * 1.2], fill=255))
    L = np.where((bottom > 0) & (yy > top + ch), cyl, L)
    lid = mask(lambda d: d.ellipse([x0, top, x0 + cw, top + ch * 0.4], fill=255))
    L = np.where(lid > 0, 0.92 - 0.3 * (xx - x0) / cw, L)
    rim = mask(lambda d: d.ellipse([x0, top, x0 + cw, top + ch * 0.4], outline=255, width=max(1, W // 120)))
    L = np.where(rim > 0, 0.08, L)
save(L, "tanks")

# --- anchor: what brings an instance back -----------------------------------------
L = np.full((H, W), BG)
ax = W * 0.5
lw = max(3, W // 28)


def anchor(d):
    d.ellipse([ax - W * 0.05, H * 0.10, ax + W * 0.05, H * 0.10 + W * 0.10], outline=255, width=lw)
    d.line([ax, H * 0.10 + W * 0.10, ax, H * 0.86], fill=255, width=lw)
    d.line([ax - W * 0.15, H * 0.34, ax + W * 0.15, H * 0.34], fill=255, width=lw)
    d.arc([ax - W * 0.27, H * 0.42, ax + W * 0.27, H * 0.88], start=10, end=170, fill=255, width=lw)
    d.polygon([(ax - W * 0.30, H * 0.60), (ax - W * 0.20, H * 0.58), (ax - W * 0.27, H * 0.50)], fill=255)
    d.polygon([(ax + W * 0.30, H * 0.60), (ax + W * 0.20, H * 0.58), (ax + W * 0.27, H * 0.50)], fill=255)


m = mask(anchor)
lightdir = 0.9 - 0.6 * (xx / W) - 0.15 * (yy / H)
L = np.where(m > 0, lightdir, L)
seabed = np.clip((yy - H * 0.80) / (H * 0.2), 0, 1) * 0.22
L = np.maximum(L, np.where(m > 0, 0, seabed))
save(L, "anchor")
