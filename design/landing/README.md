# Build device assets

This directory contains the editable Blender source and the reproducible generator for the landing-page device artwork. Geometry and materials are original to Build. Screen imagery is reconstructed demonstration fixture output from the current Build renderers, not a live-host capture.

## Build

Use Blender 4.5 LTS or newer from the repository root:

```sh
blender --background --python design/landing/build_device_assets.py
```

The canonical defaults are:

- laptop: `skriftapp/buildapp/landing/assets/screens/ui01-desktop.webp`
- tablet: `skriftapp/buildapp/landing/assets/screens/ui04-tablet.webp`
- phone: `skriftapp/buildapp/landing/assets/screens/ui02-mobile.webp`

Override them after Blender's `--` separator:

```sh
blender --background --python design/landing/build_device_assets.py -- \
  --laptop-screen /absolute/path/laptop.webp \
  --tablet-screen /absolute/path/tablet.webp \
  --phone-screen /absolute/path/phone.webp
```

The build overwrites generated output in `skriftapp/buildapp/landing/assets/devices/`, packs the Launch fixtures into `build-devices.blend`, and removes Blender's incremental `.blend1` backup.

## Runtime contract

GLBs use meters, +Y up, +Z forward, and +X right. Every GLB has one replaceable mesh node named `screen`; UI images are intentionally absent from the runtime files. Use `laptop-low.glb` for the landing-page WebGL renderer and `laptop.glb` for high-detail offline work. The runtime laptop retains its rounded keyboard keys and omits the detailed speaker grilles. Exact dimensions, screen corners, hinge pivot, triangle counts, byte sizes, render content bounds, hashes, and provenance live in `assets/devices/metadata.json`. The generated `device-contract.js` supplies the same screen dimensions to the live renderer, including the tablet's 4:3 display.

Transparent object cutouts are `hero-laptop.webp`, `mobile-hero.webp`, `laptop.webp`, `tablet.webp`, and `phone.webp`. Black-background fallbacks are emitted as six responsive pairs named `scene-01-desktop.webp` through `scene-06-desktop.webp` with matching `scene-01-mobile.webp` through `scene-06-mobile.webp`. The closing pair is also available as `desktop-poster.webp` and `mobile-poster.webp`; `social-preview.webp` is the 1200×630 sharing composition.

Scene 1 follows the runtime orthographic opening pose: camera along +Z in glTF space, model yaw −8°, pitch +4°, and transparent source framing for the hero crossfade. Cameras expand their framing when needed to leave a 5% margin around the full hardware in each responsive render. Scene 6 uses the merged 03:44 fixture consistently on every device.

The proportions, black glass, camera details, and flat graphite frames follow the reference study documented in `device-references.md`. The geometry is original, with no third-party models or Apple marks.
