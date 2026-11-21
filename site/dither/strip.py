"""Pack the pre-water rows of a scene into the source strip the site animates.

usage: strip.py PREFIX OUT.png
Reads PREFIX-lum.png, PREFIX-amber.png and PREFIX-meta.json from scene.py and writes the
rows the water reflects (sourceTop .. waterTop) as RGB: R = luminance, G = sodium light.
The meta file gains "sourceTop" so the worker knows where the strip starts.
"""
import json
import sys

import numpy as np
from PIL import Image

prefix, out = sys.argv[1], sys.argv[2]
meta = json.load(open(f"{prefix}-meta.json"))
H, top = meta["height"], meta["waterTop"]
source_top = max(0, int(top - (H - top - 1) * 0.9) - 4)
lum = np.asarray(Image.open(f"{prefix}-lum.png").convert("L"))[source_top:top]
amb = np.asarray(Image.open(f"{prefix}-amber.png").convert("L"))[source_top:top]
rgb = np.stack([lum, amb, np.zeros_like(lum)], axis=-1)
Image.fromarray(rgb, "RGB").save(out, optimize=True)
meta["sourceTop"] = source_top
json.dump(meta, open(f"{prefix}-meta.json", "w"))
