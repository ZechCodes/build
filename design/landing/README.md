# Build device assets

This directory contains the editable Blender source and the reproducible generator for the landing-page device artwork. Geometry and materials are original to Build. Screen imagery is reconstructed demonstration fixture output from the current Build renderers, not a live-host capture.

## Build

Use Blender 4.5 LTS or newer from the repository root:

```sh
blender --background --python design/landing/build_device_assets.py
```

The canonical defaults are:

- laptop: `skriftapp/buildapp/landing/assets/screens/ui01-macbook.webp`
- tablet: `skriftapp/buildapp/landing/assets/screens/ui04-ipad.webp`
- phone: `skriftapp/buildapp/landing/assets/screens/ui02-iphone.webp`

Override them after Blender's `--` separator:

```sh
blender --background --python design/landing/build_device_assets.py -- \
  --laptop-screen /absolute/path/laptop.webp \
  --tablet-screen /absolute/path/tablet.webp \
  --phone-screen /absolute/path/phone.webp
```

The build overwrites generated output in `skriftapp/buildapp/landing/assets/devices/`, packs the Launch fixtures into `build-devices.blend`, and removes Blender's incremental `.blend1` backup.

For geometry/material iteration without rebuilding the responsive poster set:

```sh
blender --background --python design/landing/build_device_assets.py -- --models-only
```

`--skip-renders` is an alias. It writes the GLBs, editable Blender source, metadata, and JavaScript contract. Use `--preview-dir /absolute/path` for transparent front/rear evidence renders and disposable GLBs.

## Runtime contract

GLBs use meters, +Y up, +Z forward, and +X right. Every GLB has one replaceable mesh node named `screen`; UI images are intentionally absent from the runtime files. `laptop-low.glb` remains the stable landing-page URL and retains the complete 78-key keyboard legends, sculpted keycaps, recessed keyboard well, and speaker perforations visible in the hero pose. Its display assembly is parented to the exported `laptop_lid` hinge so the runtime can move from closed to the authored 105° position without rotating the base. Exact body dimensions, projected overall bounds, native-aspect screen rectangles, screen-to-glass clearances, transformed screen corners, hinge endpoints, triangle counts, byte sizes, render content bounds, hashes, and provenance live in `assets/devices/metadata.json`. The generated `device-contract.js` supplies the same measurements to the live renderer.

Transparent object cutouts are `hero-laptop.webp`, `mobile-hero.webp`, `laptop.webp`, `tablet.webp`, and `phone.webp`. Black-background fallbacks are emitted as six responsive pairs named `scene-01-desktop.webp` through `scene-06-desktop.webp` with matching `scene-01-mobile.webp` through `scene-06-mobile.webp`. The closing pair is also available as `desktop-poster.webp` and `mobile-poster.webp`; `social-preview.webp` is the 1200×630 sharing composition.

Scene 1 uses a low front camera to show the keyboard depth and open display; the live hero adds only 4° of pitch. During the first 45% of that chapter the real hinge opens from closed to 105° while the base remains planted. Phone and tablet entrances use a brief pivot and then settle without idle rotation. Poster cameras expand their framing when needed to leave a 5% margin around the complete hardware. Scene 6 uses one world scale for all three devices so the 312.6 mm laptop, 249.7 mm tablet, and 78 mm phone remain physically proportional.

The generated set depicts a 2025 14-inch MacBook Pro M5, 11-inch iPad Pro M5, and iPhone 17 Pro Max. Enclosure shells use outward-facing geometry. The iPad body and screen use concentric 15.05 mm and 6.6 mm corner contours across the roughly 8.5 mm bezel. Aluminum materials use metallic 1.0 with no clearcoat, leaving the dark-strip studio environment to describe their shape. The proportions, black glass, camera details, space-black and deep-blue finishes, controls, and ports follow the reference study in `device-references.md`. The geometry is original, with no third-party models or Apple marks.
