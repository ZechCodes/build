# Landing page review build

The public landing page now presents the six chapters from the production brief with original Blender hardware, a shared scroll stage, deterministic screen checkpoints, and readable HTML close-ups. The same story remains available as ordinary sections when motion, media, or WebGL is unavailable.

Graphite surfaces use broad neutral studio lighting so the bodies, bezels, and keyboards separate from the dark background. The live renderer uses a locally generated reflection environment; the Blender posters carry matching fill and edge lighting.

The revised hardware follows the proportion study in `../landing/device-references.md`: thin flat frames, rounded black glass, camera cutouts, and a recessed laptop keyboard. The official mint Build mark appears in the public shell, favicon, social card, and screen fixtures. The tablet uses dedicated 4:3 maps and meets a matching review surface across viewport profiles.

## Preview and reproduce

From the repository root:

```sh
skriftapp/.venv/bin/python scripts/preview-landing.py
```

Open **http://127.0.0.1:4173**. This loopback preview serves the public landing, docs, privacy, installers, and assets with the production-style content security policy. The complete application owns the authenticated `/app/` pairing flow; the preview does not bypass it.

```sh
# Browser profiles; use the optional environment variables for local Chromium/GPU.
LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-check.mjs

# Re-record the two reference videos and frame measurements on a GPU browser.
LANDING_HEADFUL=1 CHROMIUM_PATH=/usr/bin/chromium node web/landing-record.mjs

# Recreate the social card after changing the Blender hero.
CHROMIUM_PATH=/usr/bin/chromium node web/landing-social.mjs
```

The reference videos are `desktop-animatic.webm` and `mobile-animatic.webm`. Their visible chapter markers correspond to `story-manifest.js`.

The final browser matrix is saved in `browser-results.json`. Still frames are `desktop-hero.png`, `phone-hero.png`, `phone-activity.png`, and `tablet-handoff.png`.

## Asset sources

- `../landing/build-devices.blend`: editable laptop, tablet, phone, lighting, cameras, and replaceable screens.
- `../landing/build_device_assets.py`: reproducible GLBs, low-detail laptop, cutouts, social composition, and twelve responsive scene posters. Its README documents dimensions, axes, pivots, licensing, and overrides.
- `../landing-captures/`: seeded Launch fixture, current product renderers, nineteen 2× PNG masters, screen-state manifest, and capture tooling. Screen textures are **reconstructed demonstration fixtures**, not footage from a live host or a claim of final product-design approval.
- `../landing-runtime/`: pinned Three.js dependency, reproducible same-origin bundle, license, and transition tests.
- `../../skriftapp/buildapp/landing/`: deployed HTML, styles, manifest, runtime, and web derivatives. Editable source, masters, and videos are not downloaded by the landing page.

## Verification

- 514 backend tests and 4,752 SPA tests after upstream integration (including 39 landing tests); six focused device-state tests.
- Python lint, JavaScript complexity cap of ten, syntax checks, and whitespace checks.
- Chromium at 1440×900, 1920×1080, 768×1024, 1024×768, 390×844, 360×740, and 844×390; separate reduced-motion and JavaScript-disabled profiles.
- GPU checks for handoff, forward/reverse checkpoints, tablet-to-review corner alignment, and the merged closing lineup.
- Keyboard menu/Escape/focus, chapter anchors, installer anchor, resizing below the story, data saving, failed model requests, WebGL context loss, and a 720×450 layout equivalent to a 1440×900 viewport at 200% browser zoom.
- Axe WCAG A/AA scan: zero reported violations on desktop hero, practical content, and mobile. Automated checks do not replace assistive-technology review.

The recorded local desktop GPU run had a median frame interval of 16.7 ms and a 95th percentile of 16.8 ms in both desktop and phone-sized browser viewports. Draw-call counts stayed unchanged while idle. These measurements describe this machine, not physical-phone performance. Raw measurements are alongside the videos.

Hero posters are about 82 KB desktop and 52 KB mobile. The three runtime GLBs total about 588 KB, including the visible laptop keyboard; all nineteen screen maps total about 468 KB. The vendored runtime is about 144 KB with gzip. `payload.json` distinguishes file measurements and calculated gzip sizes from actual production network measurements.

## Publication notes

This package records local review checks. Production verification additionally requires matching the deployed app image to the pushed commit and checking the public routes and assets after rollout. The public GitHub endpoint returns 404, so the activity section intentionally has no fabricated entries. `scripts/refresh-landing-activity.py` can populate the checked-in cache from publicly visible, labeled, merged PRs; failed refreshes preserve existing entries. The PR template documents the public summary/category fields.

Download offers the public host installer and the actual macOS/Linux host build list. Pairing and using a host still require alpha access. The page's free/open-source copy follows the supplied brief; public repository availability and the final UI/art review remain launch decisions. Performance on physical mobile devices still needs release validation.
