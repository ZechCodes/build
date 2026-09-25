"""Render the invite email's laptop: the landing's MacBook with an app capture on its display.

Run with Blender 4.5 LTS from the repository root, opening the landing's device source:
  blender --background design/landing/build-devices.blend \
    --python design/email/render_laptop.py -- \
    --screen design/email/laptop-screen.png --out /tmp/laptop-render.png

It reuses the landing generator's studio, camera fit and screen mapping, writes one
transparent PNG, and touches nothing the landing ships. `compose_laptop.py` turns that
PNG into the hosted email image.
"""

from __future__ import annotations

import argparse
import sys
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "landing"))
import build_device_assets as devices  # noqa: E402

#: Twice the email image's 1200 px width, so the compose step downsamples cleanly.
RENDER_SIZE = (2400, 1560)


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--screen", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    blender_args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    return parser.parse_args(blender_args)


def main():
    args = parse_args()
    laptop = bpy.data.collections["Laptop"]
    devices.set_screen_texture(bpy.data.materials["ScreenDesktop"], args.screen)
    devices.set_visible_collections(laptop)
    devices.render(
        args.out.resolve(), *RENDER_SIZE, devices.LAPTOP_RENDER_CAMERA,
        devices.LAPTOP_RENDER_TARGET, devices.LAPTOP_RENDER_LENS, True,
    )


if __name__ == "__main__":
    main()
