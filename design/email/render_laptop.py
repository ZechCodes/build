"""Render the invite email's laptop: the landing's MacBook with an app capture on its display.

Run with Blender 4.5 LTS from the repository root, opening the landing's device source:
  blender --background design/landing/build-devices.blend \
    --python design/email/render_laptop.py -- \
    --screen design/email/laptop-screen.png --out /tmp/laptop-render.png

It reuses the landing generator's studio, camera fit and screen mapping, writes one
transparent PNG, and touches nothing the landing ships. `compose_laptop.py` turns that
PNG into the hosted email image.

The camera sits a little below the display's centre and the lid leans 4 degrees toward
it, so the display faces the viewer and the keyboard deck is a thin foreshortened band
(about a tenth of the image) instead of half of it, as the landing's hero camera shows it.
"""

from __future__ import annotations

import argparse
import math
import sys
from pathlib import Path

import bpy

sys.path.insert(0, str(Path(__file__).resolve().parents[1] / "landing"))
import build_device_assets as devices  # noqa: E402

#: Twice the email image's 1200 px width, so the compose step downsamples cleanly.
RENDER_SIZE = (2400, 1560)
#: In the generator's decimetres; the display's centre is about 1.1 above the desk.
CAMERA = (0.0, -9.0, 0.5)
TARGET = (0.0, 0.0, 1.1)
LENS = 70
#: The lid hinge's rotation: 0 stands the display upright, the landing's -15 leans it back.
LID_ANGLE_DEG = 4.0


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
    bpy.data.objects["laptop_lid"].rotation_euler.x = math.radians(LID_ANGLE_DEG)
    devices.render(args.out.resolve(), *RENDER_SIZE, CAMERA, TARGET, LENS, True)


if __name__ == "__main__":
    main()
