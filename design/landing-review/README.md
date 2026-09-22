# Landing page review build

The current device geometry and motion revision, screenshots, and validation are recorded in [devices-2025/README.md](devices-2025/README.md).

The public landing page presents the six chapters from the production brief with original Blender hardware, a shared scroll stage, deterministic screen checkpoints, and readable HTML close-ups. The same story remains available as ordinary sections when motion, media, or WebGL is unavailable.

Graphite surfaces use neutral studio strips separated by dark regions so the bodies, bezels, and keyboards remain legible against the background. Aluminum exports use metallic 1.0 and clearcoat 0.0. The live renderer builds its reflection environment locally and uses a 20° perspective camera with a 0.5-unit near plane; Blender posters use related fill and edge lighting.

The hardware depicts the 2025 MacBook Pro 14-inch M5, iPad Pro 11-inch M5, and iPhone 17 Pro Max using the references in `../landing/device-references.md`. The shells face outward, the laptop has raised dished keys over a recessed solid deck and a compact curved sidewall, and the iPad's 15.05 mm body and 6.6 mm screen contours share their corner centers. Native-aspect screens include persistent macOS, iPadOS, and iOS interface regions. The official mint Build mark appears in the public shell, favicon, social card, and screen fixtures.

The live hero uses a low, nearly straight-on view with 4° of pitch. Its named hinge opens the lid from closed to the authored 105° position during local progress 0 through 0.45 while the base stays planted. Phone and tablet arrivals pivot into place, then stop rotating.

During partial-opacity handoffs, each device receives a nearest-surface depth pass before color. The renderer clears depth between devices while retaining previously drawn color and a stable laptop, phone, tablet layer order. This keeps the laptop deck solid and prevents a nearly invisible arrival from cutting through hardware behind it. Single opaque-device frames retain the one-pass render path.

Active screen pixels use an unlit texture material with tone mapping disabled. Dark app content therefore remains stable as hardware moves through the reflection environment, while a subdued front-glass layer leaves highlights on the bezel and metal. The refreshed device stills in `devices-2025/` show this final treatment; the retained recordings and geometry-focused captures predate the glare-only change.

## Preview and reproduce

From the repository root:

```sh
skriftapp/.venv/bin/python scripts/preview-landing.py
```

Open **http://127.0.0.1:4173**. This loopback preview serves the public landing, docs, privacy, installers, and assets with the production-style content security policy. The complete application owns the authenticated `/app/` pairing flow; the preview does not bypass it.

```sh
# Browser profiles; use the optional environment variables for local Chromium/GPU.
LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-check.mjs

# Re-record this revision's two videos and frame measurements on a GPU browser.
LANDING_REVIEW_DIR=design/landing-review/devices-2025 \
  LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-record.mjs

# Recreate the social card after changing the Blender hero.
CHROMIUM_PATH=/usr/bin/chromium node web/landing-social.mjs
```

The current reference videos are `devices-2025/desktop-animatic.webm` and `devices-2025/mobile-animatic.webm`. Their visible chapter markers correspond to `story-manifest.js`. `LANDING_REVIEW_DIR` controls where the recorder writes videos and performance JSON; static mobile profiles do not wait for a WebGL-ready signal.

The focused browser matrix and stills are saved under `devices-2025/`. The earlier integration matrix and still frames remain at this directory's top level.

## Asset sources

- `../landing/build-devices.blend`: editable laptop, tablet, phone, lighting, cameras, and replaceable screens.
- `../landing/build_device_assets.py`: reproducible GLBs, low-detail laptop, cutouts, social composition, and twelve responsive scene posters. Its README documents dimensions, axes, pivots, licensing, and overrides.
- `../landing-captures/`: seeded Launch fixture, current product renderers, twenty-four native-resolution system-screen PNG masters, retained app-only archives, the screen-state manifest, and capture tooling. Screen textures are **reconstructed demonstration fixtures**, not footage from a live host or a claim of final product-design approval.
- `../landing-runtime/`: pinned Three.js dependency, reproducible same-origin bundle, license, and transition tests.
- `../../skriftapp/buildapp/landing/`: deployed HTML, styles, manifest, runtime, and web derivatives. Editable source, masters, and videos are not downloaded by the landing page.

## Original story verification

The following records cover the earlier story-integration review. The current device revision has its own focused validation and browser captures in `devices-2025/`.

- 521 backend tests and 4,752 SPA tests after upstream integration (including 39 landing tests); six focused device-state tests.
- Python lint, JavaScript complexity cap of ten, syntax checks, and whitespace checks.
- Production rate limiting gives `GET /landing/` its own bounded 600/minute per-IP budget, preventing full-scroll asset fanout from exhausting the 60/minute default.
- Chromium at 1440×900, 1920×1080, 768×1024, 1024×768, 390×844, 360×740, and 844×390; separate reduced-motion and JavaScript-disabled profiles.
- GPU checks for handoff, forward/reverse checkpoints, tablet-to-review corner alignment, and the merged closing lineup.
- Keyboard menu/Escape/focus, chapter anchors, installer anchor, resizing below the story, data saving, failed model requests, WebGL context loss, and a 720×450 layout equivalent to a 1440×900 viewport at 200% browser zoom.
- Axe WCAG A/AA scan: zero reported violations on desktop hero, practical content, and mobile. Automated checks do not replace assistive-technology review.

The retained local desktop GPU run held idle draw calls at 84 and measured a 16.7 ms median and 16.8 ms p95 across 1,432 frame intervals, with two intervals over 34 ms. The 390×844 recording used the static document path with zero WebGL draws and measured 16.7 ms median and 16.8 ms p95 across 1,439 intervals, with none over 34 ms. These measurements describe this machine and the preceding geometry/motion revision, not physical-phone performance. Raw measurements and videos are in `devices-2025/`.

The current hero cutouts are 53 KB desktop and 55 KB mobile. The three runtime GLBs total 2.63 MB, the 24 system screen maps total 786 KB, and the vendored runtime is 145 KB with gzip. `payload.json` records the exact local file and calculated gzip sizes; these are not production network measurements.

## Publication notes

This package records local review checks. Production verification additionally requires matching the deployed app image to the pushed commit and checking the public routes and assets after rollout. The public GitHub endpoint returns 404, so the activity section intentionally has no fabricated entries. `scripts/refresh-landing-activity.py` can populate the checked-in cache from publicly visible, labeled, merged PRs; failed refreshes preserve existing entries. The PR template documents the public summary/category fields.

Download offers the public host installer and the actual macOS/Linux host build list. Pairing and using a host still require alpha access. The page's free/open-source copy follows the supplied brief; public repository availability and the final UI/art review remain launch decisions. Performance on physical mobile devices still needs release validation.
