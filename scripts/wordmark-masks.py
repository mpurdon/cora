#!/usr/bin/env python3
"""Turn a full-colour wordmark export into the two alpha masks the rail header
paints from.

Not part of the build — run it by hand when the artwork changes. Needs Pillow
and numpy, which nothing else here depends on:

    pip install Pillow numpy
    python3 scripts/wordmark-masks.py ~/Desktop/header.png

Why masks instead of the picture: a flat PNG carries its own colours *and* its
own near-black fills, so on any surface that isn't the one it was drawn for it
sits there as a sticker. Mapping luminance onto opacity turns those fills into
holes the background shows through, and splitting the remaining ink by
saturation lets the two colours come from CSS variables. Theme support is then
two variable values rather than a second export.

Writes three files into src/assets:

    cora-wordmark.png              trimmed, resized, full colour — kept only so
                                   the masks can be regenerated from the repo
    cora-wordmark-mask-light.png   lettering, rules, line art  -> var(--text)
    cora-wordmark-mask-warm.png    chevrons, checks, bolt      -> var(--brand-warm)
"""

import argparse
import pathlib
import sys

import numpy as np
from PIL import Image

ASSETS = pathlib.Path(__file__).resolve().parent.parent / "src" / "assets"

# The widest the mark can render is the 520px maximum rail less its padding,
# and the webview can be at 1.6x zoom on a 2x display. 496 * 3.2 rounds up to
# here, so the masks never resample upwards.
DEFAULT_WIDTH = 1200


def trim(im: Image.Image) -> Image.Image:
    """Crop to the visible ink.

    Not `getbbox()`: these exports carry a soft black drop shadow that reaches
    the canvas edge, so alpha alone finds almost no margin. What matters is
    where there is ink bright enough to survive becoming opacity, since
    anything darker ends up transparent anyway.
    """
    a = np.asarray(im).astype(np.float32)
    visible = (a[..., 3] > 20) & (a[..., :3].sum(-1) > 60)
    if not visible.any():
        sys.exit("nothing visible in that image — is it all transparent?")
    ys, xs = np.where(visible)
    return im.crop((int(xs.min()), int(ys.min()), int(xs.max()) + 1, int(ys.max()) + 1))


def smoothstep(lo: float, hi: float, x: np.ndarray) -> np.ndarray:
    t = np.clip((x - lo) / (hi - lo), 0, 1)
    return t * t * (3 - 2 * t)


def masks(im: Image.Image) -> dict[str, np.ndarray]:
    """Split the artwork into a cool layer and a warm one.

    Both are opacity maps: luminance decides how much ink lands, and saturation
    decides which layer it lands in. The split is a ramp rather than a
    threshold so the two recompose without a seam along the boundary.
    """
    a = np.asarray(im).astype(np.float32)
    rgb, alpha = a[..., :3] / 255.0, a[..., 3] / 255.0
    lum = 0.2126 * rgb[..., 0] + 0.7152 * rgb[..., 1] + 0.0722 * rgb[..., 2]
    mx, mn = rgb.max(-1), rgb.min(-1)
    sat = np.where(mx > 0, (mx - mn) / np.maximum(mx, 1e-6), 0)

    ink = alpha * lum
    warm = smoothstep(0.22, 0.45, sat)
    return {
        "cora-wordmark-mask-warm.png": ink * warm,
        "cora-wordmark-mask-light.png": ink * (1 - warm),
    }


def write_mask(path: pathlib.Path, opacity: np.ndarray) -> None:
    out = np.zeros((*opacity.shape, 4), np.uint8)
    out[..., :3] = 255  # the colour comes from CSS; only alpha is read
    out[..., 3] = np.clip(opacity * 255, 0, 255).astype(np.uint8)
    Image.fromarray(out, "RGBA").save(path, optimize=True)


def main() -> None:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument(
        "source",
        nargs="?",
        default=ASSETS / "cora-wordmark.png",
        type=pathlib.Path,
        help="full-colour wordmark export (default: the copy in src/assets)",
    )
    ap.add_argument("--width", type=int, default=DEFAULT_WIDTH)
    args = ap.parse_args()

    im = trim(Image.open(args.source).convert("RGBA"))
    height = round(args.width * im.height / im.width)
    im = im.resize((args.width, height), Image.LANCZOS)

    im.save(ASSETS / "cora-wordmark.png", optimize=True)
    for name, opacity in masks(im).items():
        write_mask(ASSETS / name, opacity)

    for name in ("cora-wordmark.png", "cora-wordmark-mask-light.png", "cora-wordmark-mask-warm.png"):
        print(f"  {name:<30} {(ASSETS / name).stat().st_size // 1024:>4} KB")
    print(f"\nIf the proportions changed, styles.css needs: aspect-ratio: {args.width} / {height};")


if __name__ == "__main__":
    main()
