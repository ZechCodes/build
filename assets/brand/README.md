# Build brand assets

The canonical source artwork is `build-mark.svg`. It has a transparent
background and uses Build mint `#5EF5B5`. The root `recreated-vector.svg` is a
byte-for-byte compatibility copy of this artwork. Edit the canonical file when
updating the mark; the generator reads its geometry and viewBox directly.

The square variants use these colors:

| Asset | Mark | Background |
| --- | --- | --- |
| `build-black-on-mint` | `#000000` | `#5EF5B5` |
| `build-mint-on-black` | `#5EF5B5` | `#000000` |
| `build-black-on-white` | `#000000` | `#FFFFFF` |

Each variant is provided as SVG and a 1024×1024 PNG. The source geometry is
uniformly scaled and centered on a square canvas. The mark occupies 64% of the
canvas height, leaving 18% padding above and below. Its proportions and
negative space remain unchanged, so the cutout shows the background color.

Create a local Python environment and install the generation dependencies:

```sh
python3 -m venv .venv
.venv/bin/python -m pip install CairoSVG Pillow
```

Then run the generator from the repository root:

```sh
.venv/bin/python scripts/generate-brand-assets.py
```

The generator recreates the brand variants and the runtime icons used by the
web, landing, and desktop apps. It does not rewrite the canonical source.

Runtime output paths are:

- `spa/public/favicon.svg`, `icon-192.png`, `icon-512.png`, and
  `apple-touch-icon.png`
- `skriftapp/buildapp/landing/favicon.svg`, `favicon.png`, and `brand-mark.svg`
- `desktop/assets/icon.png`, `icon.ico`, and `icon.icns`

Generation was verified with CairoSVG 2.9.1 and Pillow 12.3.0.

The landing page uses the transparent mark and black-on-mint favicon from these
canonical assets. The screen-capture fixture keeps an identical transparent
mark at `design/landing-captures/build-mark.svg`. No geometry or color changes
were made for the landing page.
