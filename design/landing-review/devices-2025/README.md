# 2025 device geometry and motion review

The landing page now depicts the 14-inch MacBook Pro M5 (2025), 11-inch iPad Pro M5 (2025), and iPhone 17 Pro Max. Apple specification pages and reference drawings are linked in [the hardware reference study](../../landing/device-references.md).

The editable Blender source and generator produce the runtime models and fallback posters. Enclosure shells use outward-facing geometry. The laptop includes an exported 105° hinge, raised tapered keys with a shallow dish, a solid deck with a cut keyboard recess, legends, Touch ID, inverted-T arrows, speaker perforations, a flush trackpad, and ports seated on the enclosure surface. Its 16-ring sidewall profile uses a compact upper curve, a short straight port band, and an eight-segment lower ellipse measuring 1.8 mm outward by 4 mm vertically instead of a tall flat face. The iPad uses concentric 15.05 mm body and 6.6 mm screen contours across its roughly 8.5 mm bezel. Tablet and phone models use the documented chassis dimensions, native display ratios, controls, and camera details.

Aluminum exports use metallic 1.0 and clearcoat 0.0. The live renderer shapes them with bright studio strips separated by a dark environment, a 20° perspective camera, and a 0.5-unit near plane. The hero stays in a low, nearly straight-on view with 4° of pitch. Its physical lid opens from closed to 105° over local progress 0 through 0.45 while the base remains planted. Phone and tablet arrivals pivot into place and then settle without idle rotation.

Partially transparent hardware renders its own nearest-surface depth pass before its color pass, preventing rear shell, keyboard, and hinge fragments from showing through the deck during handoff. Multi-device frames retain the fixed laptop, phone, tablet layer order at opacity endpoints; only depth is cleared between devices, so an arriving device does not erase color already drawn behind it. A single opaque device keeps the original one-pass path.

Active display pixels use an unlit texture material with tone mapping disabled, so dark app content stays stable while a device turns through the studio environment. The restrained front-glass response keeps highlights on the bezel and metal instead of washing across the interface. The Mac screen includes the menu bar, a window title bar, and Dock. iPad uses its status bar, iPadOS window controls, Dock, and home indicator. iPhone reserves the status/Dynamic Island area and home area. App images never extend into those regions. While a new checkpoint texture loads, the previous complete display remains visible, including its system UI.

## Review captures

- [Closed opening state](opening-closed.png)
- [Half-open hinge state](opening-half.png)
- [Stable active pixels under the studio environment](screen-glare-fixed.png)
- [Settled MacBook hero](macbook.png)
- [Laptop and phone handoff](handoff.png)
- [Handoff chassis detail](chassis-handoff-detail.png)
- [Low-angle chassis profile](chassis-low-angle.png)
- [Partial-opacity alpha evidence](handoff-alpha.png)
- [iPhone conversation](iphone.png)
- [iPad activity](ipad.png)
- [Landscape iPad layout](ipad-landscape-layout.png)
- [Physical-scale closing lineup](lineup.png)
- [Mobile layout](mobile.png)
- [Keyboard geometry detail](keyboard-detail.png)
- [Concentric iPad corners](ipad-corners.png)
- [Desktop motion recording](desktop-animatic.webm)
- [Mobile document recording](mobile-animatic.webm)

These captures show the production page at 1440×900 or 390×844 in Chromium on an NVIDIA RTX 4060 Ti. Visual review covers silhouettes, outward-facing edges, keyboard depth, the deck recess, camera cutouts, display proportions, and system regions against the documented references. The opening offline poster uses the same low front hardware presentation as the hero; later posters frame each chapter's device set and screen state independently.

The targeted glare capture and refreshed MacBook, iPhone, iPad, landscape-tablet, and lineup stills show the final screen treatment; the mobile still shows the current document layout. The animatics, performance JSON, opening states, handoff captures, and geometry details predate the glare-only material change; they remain evidence for motion, timing, opacity, and physical geometry rather than the final active-pixel appearance.

The handoff captures isolate the geometry and transparency correction: the deck remains solid, the rolled lower enclosure replaces the former flat wall, and the ports meet the straight side band. At 0.58 model opacity, the alpha evidence records a single 148/255 hardware layer instead of accumulated rear and interior surfaces.

The local desktop recording held idle draw calls at 84 before and after the sample. Across 1,432 frame intervals it measured 16.7 ms median and 16.8 ms p95, with two intervals over 34 ms. The 390×844 layout uses the static document path, so it held WebGL draws at zero; its 1,439 sampled intervals measured 16.7 ms median and 16.8 ms p95 with none over 34 ms. These measurements describe this review machine, not physical-phone performance. Raw values are in [desktop-performance.json](desktop-performance.json) and [mobile-performance.json](mobile-performance.json).

## Verification

| Requirement | Current evidence |
| --- | --- |
| Correct device sizes, display ratios, and depth | Generated `device-contract.js` and `metadata.json`; exported-GLB tests verify native aspect, unobstructed displays, outward side winding, screen-to-glass clearance, raised dished keys, the keyboard recess, and concentric iPad corners. |
| Handoff surfaces remain solid | Focused renderer tests verify per-device depth and color passes, color preservation across depth clears, stable multi-device layer order at opacity endpoints, and the single-pass opaque path. |
| Active pixels stay readable while hardware turns | [Targeted glare capture](screen-glare-fixed.png) plus runtime tests verify an unlit, untone-mapped active texture and a restrained reflection contract limited to front glass. |
| App content stays outside persistent system UI | [Capture results](../../landing-captures/capture-results.json): 24 native masters and runtime maps, with measured app bounds and system exclusions; zero issues. |
| System UI remains present across content changes | `web/landing-check.mjs` holds a phone texture request and verifies the previous display stays ready without a poster flash, then verifies the replacement arrives. |
| All page layouts and fallbacks remain usable | [Browser results](browser-results.json): 7 viewport profiles, reduced motion, and JavaScript disabled; forward/reverse seeks, checkpoint order, resolved images, no horizontal overflow, no browser errors. |
| Source and generated models stay consistent | Runtime tests: 24 passed, including physical hinge endpoints, exact review projection, landscape-tablet framing, solid partial-opacity rendering, and screen-material isolation. SPA story/device tests: 15 passed. Landing backend tests: 51 passed. Ruff, JavaScript complexity cap 10, syntax, and whitespace checks passed. |

Regenerate the fixtures before Blender output when changing screen design:

```sh
spa/node_modules/.bin/vite . --host 127.0.0.1 --port 4178
CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
blender --background --python design/landing/build_device_assets.py
```

Run the browser matrix with the preview server running:

```sh
skriftapp/.venv/bin/python scripts/preview-landing.py
LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-check.mjs

LANDING_REVIEW_DIR=design/landing-review/devices-2025 \
  LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-record.mjs
```

The three runtime models total 2.63 MB and load as their chapters approach; the 24 system screen maps total 786 KB. Screens are demonstration fixtures and the geometry is original. Exact model and poster sizes are recorded in [payload.json](../payload.json) as local file measurements, not deployment network measurements.
