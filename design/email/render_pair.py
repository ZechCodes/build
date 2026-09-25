"""Render the invite email's device pair: the landing's iPad and phone, tipped away from each other.

Run with Blender 4.5 LTS from the repository root, opening the landing's device source:
  blender --background design/landing/build-devices.blend \
    --python design/email/render_pair.py -- \
    --tablet-screen design/email/tablet-screen.png \
    --phone-screen design/email/phone-screen.png --out /tmp/pair-render.png

The tablet stands left and the phone in front of its right edge, overlapping it slightly.
Each turns the opposite way about the vertical axis, so the edges where they meet come
forward and the outer edges fall back. It reuses the landing generator's studio, camera
fit and screen mapping, writes one transparent PNG, and saves nothing back to the .blend.
`compose_hero.py` turns that PNG into the hosted email image.
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
RENDER_SIZE = (2400, 1400)
#: Positions in the generator's decimetres; the phone stands 0.8 nearer the camera.
TABLET_OFFSET = (-0.45, 0.0, 0.0)
PHONE_OFFSET = (0.95, -0.8, -0.2)
#: About the vertical axis: the tablet's screen turns left, the phone's right.
TURN_DEG = 20.0
CAMERA = (0.0, -8.0, 0.6)
TARGET = (0.0, 0.0, -0.05)
LENS = 70


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tablet-screen", type=Path, required=True)
    parser.add_argument("--phone-screen", type=Path, required=True)
    parser.add_argument("--out", type=Path, required=True)
    blender_args = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    return parser.parse_args(blender_args)


def main():
    args = parse_args()
    tablet, phone = bpy.data.collections["Tablet"], bpy.data.collections["Phone"]
    devices.set_screen_texture(bpy.data.materials["ScreenTablet"], args.tablet_screen)
    devices.set_screen_texture(bpy.data.materials["ScreenPhone"], args.phone_screen)
    turn = math.radians(TURN_DEG)
    devices.transform_collection(tablet, (0, 0, 0), offset=TABLET_OFFSET, rotation=(0, 0, turn))
    devices.transform_collection(phone, (0, 0, 0), offset=PHONE_OFFSET, rotation=(0, 0, -turn))
    devices.set_visible_collections(tablet, phone)
    devices.render(args.out.resolve(), *RENDER_SIZE, CAMERA, TARGET, LENS, True)


if __name__ == "__main__":
    main()
