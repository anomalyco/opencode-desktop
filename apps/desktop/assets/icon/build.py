"""Build ../icon.icns from art-512.png: the full-bleed art placed on Apple's macOS icon grid
(body = 824/1024 of the canvas, centred, soft drop shadow beneath). Run: uv run --with pillow --with numpy --with scipy python build.py"""
import os, subprocess, tempfile
import numpy as np
from PIL import Image
from scipy import ndimage as ndi

HERE = os.path.dirname(os.path.abspath(__file__))
ART = Image.open(os.path.join(HERE, "art-512.png")).convert("RGBA")
BODY = 824 / 1024


def render(s):
    body = round(s * BODY)
    art = ART.resize((body, body), Image.LANCZOS)
    out = Image.new("RGBA", (s, s), (0, 0, 0, 0))
    off = (s - body) // 2
    a = np.zeros((s, s))
    a[off:off + body, off:off + body] = np.asarray(art)[..., 3] / 255
    sh = ndi.gaussian_filter(ndi.shift(a, (12 * s / 1024, 0), order=1), 12 * s / 1024) * 0.35
    out.putalpha(Image.fromarray((sh * 255 + 0.5).astype(np.uint8)))
    out.alpha_composite(art, (off, off))
    return out


slots = {"16x16": 16, "16x16@2x": 32, "32x32": 32, "32x32@2x": 64, "128x128": 128, "128x128@2x": 256,
         "256x256": 256, "256x256@2x": 512, "512x512": 512, "512x512@2x": 1024}
with tempfile.TemporaryDirectory() as t:
    d = os.path.join(t, "icon.iconset")
    os.mkdir(d)
    for name, s in slots.items():
        render(s).save(os.path.join(d, f"icon_{name}.png"))
    subprocess.run(["iconutil", "-c", "icns", d, "-o", os.path.join(HERE, "..", "icon.icns")], check=True)
render(1024).save(os.path.join(HERE, "icon-1024.png"))
