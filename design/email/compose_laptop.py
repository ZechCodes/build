"""Turn the transparent Blender render into the invite email's laptop image.

Run with Pillow (the app's venv has it) from the repository root:
  skriftapp/.venv/bin/python design/email/compose_laptop.py \
    /tmp/laptop-render.png skriftapp/buildapp/landing/email/laptop.png

Trims the render to the hardware, pads it evenly, lays it on the email's background
colour so no client ever shows transparency, scales it to 1200 px (560 px displayed, over
2x) and quantizes it to a palette so it stays well under 250 KB.
"""

from __future__ import annotations

import sys
from pathlib import Path

from PIL import Image

#: EMAIL_BACKGROUND in skriftapp/buildapp/email_template.py.
EMAIL_BACKGROUND = (0x03, 0x06, 0x04)
OUTPUT_WIDTH_PX = 1200
#: Space around the hardware, as a fraction of its width, before scaling.
PADDING_FRACTION = 0.03
PALETTE_COLOURS = 256


def compose(render: Image.Image) -> Image.Image:
    hardware = render.crop(render.getchannel("A").getbbox())
    padding = round(hardware.width * PADDING_FRACTION)
    canvas = Image.new(
        "RGB",
        (hardware.width + 2 * padding, hardware.height + 2 * padding),
        EMAIL_BACKGROUND,
    )
    canvas.paste(hardware, (padding, padding), hardware)
    height = round(canvas.height * OUTPUT_WIDTH_PX / canvas.width)
    scaled = canvas.resize((OUTPUT_WIDTH_PX, height), Image.Resampling.LANCZOS)
    # An octree palette keeps the small saturated colours (the dock's icons) that a
    # median cut spends on the dark gradients; the dithered remap hides the banding.
    palette = scaled.quantize(PALETTE_COLOURS, method=Image.Quantize.FASTOCTREE)
    return scaled.quantize(palette=palette, dither=Image.Dither.FLOYDSTEINBERG)


def main(source: Path, target: Path) -> None:
    target.parent.mkdir(parents=True, exist_ok=True)
    compose(Image.open(source).convert("RGBA")).save(target, optimize=True)
    with Image.open(target) as written:
        print(f"{target}: {written.width}x{written.height}, {target.stat().st_size} bytes")


if __name__ == "__main__":
    main(Path(sys.argv[1]), Path(sys.argv[2]))
