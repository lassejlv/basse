"""Overlay the dithered amber layer onto the dithered scene layer.

usage: composite.py SCENE.png AMBER.png OUT.png
Pixels that dith set to amber in the amber layer win; every other pixel keeps
the scene layer's colour, so the output stays inside the exact palette.
"""
import sys

import numpy as np
from PIL import Image

scene = np.asarray(Image.open(sys.argv[1]).convert("RGB")).copy()
amber = np.asarray(Image.open(sys.argv[2]).convert("RGB"))
lit = (amber[..., 0] == 0xF5) & (amber[..., 1] == 0xA5) & (amber[..., 2] == 0x24)
scene[lit] = amber[lit]
Image.fromarray(scene, "RGB").save(sys.argv[3])
