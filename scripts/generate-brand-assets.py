#!/usr/bin/env python3
"""Generate Build brand variants and application icon files."""

from __future__ import annotations

import io
from copy import deepcopy
from pathlib import Path
from xml.etree import ElementTree

import cairosvg
from PIL import Image

ElementTree.register_namespace("", "http://www.w3.org/2000/svg")


ROOT = Path(__file__).resolve().parents[1]
BRAND_DIR = ROOT / "assets" / "brand"
SOURCE = BRAND_DIR / "build-mark.svg"

CANVAS_SIZE = 1024
MARK_HEIGHT = CANVAS_SIZE * 0.64
MINT = "#5EF5B5"
BLACK = "#000000"
WHITE = "#FFFFFF"

VARIANTS = {
    "build-black-on-mint": (BLACK, MINT),
    "build-mint-on-black": (MINT, BLACK),
    "build-black-on-white": (BLACK, WHITE),
}


def read_source_geometry() -> tuple[tuple[float, float, float, float], str]:
    source_root = ElementTree.parse(SOURCE).getroot()
    viewbox = tuple(float(value) for value in source_root.attrib["viewBox"].split())
    if len(viewbox) != 4:
        raise ValueError(f"Expected four values in {SOURCE} viewBox")

    geometry = []
    for source_element in source_root:
        element = deepcopy(source_element)
        element.tail = None
        for descendant in element.iter():
            descendant.tag = descendant.tag.rsplit("}", 1)[-1]
            if descendant.attrib.get("fill", "").lower() != "none":
                descendant.attrib.pop("fill", None)
        geometry.append(ElementTree.tostring(element, encoding="unicode"))
    if not geometry:
        raise ValueError(f"No geometry found in {SOURCE}")
    return viewbox, "\n    ".join(geometry)


def variant_svg(
    foreground: str,
    background: str,
    viewbox: tuple[float, float, float, float],
    geometry: str,
) -> str:
    source_x, source_y, source_width, source_height = viewbox
    scale = MARK_HEIGHT / source_height
    translate_x = (CANVAS_SIZE - source_width * scale) / 2 - source_x * scale
    translate_y = (CANVAS_SIZE - source_height * scale) / 2 - source_y * scale
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="1024" height="1024" viewBox="0 0 1024 1024">
  <rect width="1024" height="1024" fill="{background}"/>
  <g fill="{foreground}" transform="matrix({scale:.12f} 0 0 {scale:.12f} {translate_x:.12f} {translate_y:.12f})">
    {geometry}
  </g>
</svg>
'''


def render_svg(svg: str, size: int) -> Image.Image:
    png = cairosvg.svg2png(
        bytestring=svg.encode("utf-8"), output_width=size, output_height=size
    )
    with Image.open(io.BytesIO(png)) as image:
        return image.convert("RGB")


def save_png(image: Image.Image, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    image.save(destination, format="PNG", optimize=True)


def generate_brand_variants() -> dict[str, str]:
    BRAND_DIR.mkdir(parents=True, exist_ok=True)
    viewbox, geometry = read_source_geometry()

    svgs: dict[str, str] = {}
    for name, (foreground, background) in VARIANTS.items():
        svg = variant_svg(foreground, background, viewbox, geometry)
        svgs[name] = svg
        (BRAND_DIR / f"{name}.svg").write_text(svg, encoding="utf-8")
        save_png(render_svg(svg, CANVAS_SIZE), BRAND_DIR / f"{name}.png")
    return svgs


def generate_runtime_assets(default_svg: str) -> None:
    default_image = render_svg(default_svg, CANVAS_SIZE)

    spa_public = ROOT / "spa" / "public"
    spa_public.mkdir(parents=True, exist_ok=True)
    (spa_public / "favicon.svg").write_text(default_svg, encoding="utf-8")
    for filename, size in (
        ("icon-192.png", 192),
        ("icon-512.png", 512),
        ("apple-touch-icon.png", 180),
    ):
        save_png(default_image.resize((size, size), Image.Resampling.LANCZOS), spa_public / filename)

    landing = ROOT / "skriftapp" / "buildapp" / "landing"
    landing.mkdir(parents=True, exist_ok=True)
    landing.joinpath("brand-mark.svg").write_bytes(SOURCE.read_bytes())
    (landing / "favicon.svg").write_text(default_svg, encoding="utf-8")
    save_png(default_image.resize((32, 32), Image.Resampling.LANCZOS), landing / "favicon.png")

    desktop_assets = ROOT / "desktop" / "assets"
    desktop_assets.mkdir(parents=True, exist_ok=True)
    save_png(default_image, desktop_assets / "icon.png")
    default_image.save(
        desktop_assets / "icon.ico",
        format="ICO",
        sizes=[(size, size) for size in (16, 24, 32, 48, 64, 128, 256)],
    )
    default_image.save(
        desktop_assets / "icon.icns",
        format="ICNS",
        append_images=[
            default_image.resize((size, size), Image.Resampling.LANCZOS)
            for size in (16, 32, 64, 128, 256, 512)
        ],
    )


def main() -> None:
    if not SOURCE.is_file():
        raise FileNotFoundError(f"Source mark not found: {SOURCE}")
    variants = generate_brand_variants()
    generate_runtime_assets(variants["build-black-on-mint"])


if __name__ == "__main__":
    main()
