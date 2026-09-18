# Build brand assets

These assets are copied unchanged from Build branding commit `a96359e6`
(`Bundle official brand assets and update application icons`). The canonical
source artwork is `build-mark.svg`. It has a transparent background and uses
Build mint `#5EF5B5`.

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

The landing page serves the transparent mark from
`skriftapp/buildapp/landing/brand-mark.svg` and the black-on-mint favicon from
`favicon.svg` and `favicon.png` in that directory. The screen-capture fixture
keeps an identical copy of the transparent mark at
`design/landing-captures/build-mark.svg`.

The shared generation script, `scripts/generate-brand-assets.py`, is retained
in the source branding commit. Its original generation used CairoSVG 2.9.1
and Pillow 12.3.0. No geometry or color changes were made for this landing page.
