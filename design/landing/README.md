# Build device assets

This directory contains the editable Blender source and the reproducible generator for the landing-page device artwork. Geometry and materials are original to Build. Screen imagery is reconstructed demonstration fixture output from the current Build renderers, not a live-host capture.

## Build

Use Blender 4.5 LTS or newer from the repository root:

```sh
blender --background --python design/landing/build_device_assets.py
```

The social renderer loads the checked-in `fonts/Inter-Bold.ttf`, derived from
`spa/node_modules/@fontsource/inter/files/inter-latin-700-normal.woff2` with
`fontTools.ttLib.removeOverlaps`. Removing the overlapping glyph contours lets Blender fill
the headline cleanly; the source font's license is stored beside the generated TTF.

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

Landing-story renders use the checked-in geometry without exporting models or resaving the
Blender source. Open the source explicitly, then choose one independently committable group:

```sh
blender --background design/landing/build-devices.blend \
  --python design/landing/build_device_assets.py -- --render-only posters
blender --background design/landing/build-devices.blend \
  --python design/landing/build_device_assets.py -- --render-only cutouts
blender --background design/landing/build-devices.blend \
  --python design/landing/build_device_assets.py -- --render-only social
```

`--render-only all` renders the same three groups in one run. Render-only mode updates render
bounds, hashes, and screen provenance in `assets/devices/metadata.json`; it does not write the
GLBs, `device-contract.js`, or `build-devices.blend`.

Use `--render-only closing` to update only Act 8's desktop and mobile stills and their
byte-identical poster aliases. It keeps the source Blender file, models, and other scenes
untouched while updating those four metadata records.

The cutout group includes `hero-laptop` and the tablet/phone `mobile-hero` composition as well
as the three individual devices. The complete document set has 25 render records, including
the two closing aliases and the PNG social preview. `hero-entrance.webp` is separate: it is
captured from the live stage by `web/capture-hero-entrance.mjs` and is never overwritten here.

The build overwrites generated output in `skriftapp/buildapp/landing/assets/devices/`, packs the Launch fixtures into `build-devices.blend`, and removes Blender's incremental `.blend1` backup.

For geometry/material iteration without rebuilding the responsive poster set:

```sh
blender --background --python design/landing/build_device_assets.py -- --models-only
```

`--skip-renders` is an alias. It writes the GLBs, editable Blender source, metadata, and JavaScript contract. Use `--preview-dir /absolute/path` for transparent front/rear evidence renders and disposable GLBs.

## Runtime contract

GLBs use meters, +Y up, +Z forward, and +X right. Every GLB has one replaceable mesh node named `screen`; UI images are intentionally absent from the runtime files. `laptop-low.glb` remains the stable landing-page URL and retains the complete 78-key keyboard legends, sculpted keycaps, recessed keyboard well, and speaker perforations visible in the hero pose. Its display assembly is parented to the exported `laptop_lid` hinge so the runtime can move from closed to the authored 105° position without rotating the base. Exact body dimensions, projected overall bounds, native-aspect screen rectangles, screen-to-glass clearances, transformed screen corners, hinge endpoints, triangle counts, byte sizes, render content bounds, hashes, and provenance live in `assets/devices/metadata.json`. The generated `device-contract.js` supplies the same measurements to the live renderer.

Transparent object cutouts are `hero-laptop.webp`, `mobile-hero.webp`, `laptop.webp`, `tablet.webp`, and `phone.webp`. The document cutouts show `ui10-editor` on the laptop, `ui05-merged` on the tablet, and `ui03-answer` on the phone. Black-background fallbacks are emitted as eight responsive pairs named `scene-01-desktop.webp` through `scene-08-desktop.webp` with matching mobile files. The closing aliases `desktop-poster.webp` and `mobile-poster.webp` are byte-identical copies of scene 08. `social-preview.webp` and `assets/social-preview.png` are matching 1200×630 sharing compositions with the editor laptop and “Your agents. Your machine. Your call.” headline.

Scene 1 uses a low front camera to show the keyboard depth and fully open display; the live hero adds only 4° of pitch. Its independent runtime entrance starts at least three-quarters open and settles to 105° while the base remains planted. The rendered fallback remains fully open. Phone and tablet entrances use a brief pivot and then settle without idle rotation. Poster cameras expand their framing when needed to leave a 5% margin around the complete hardware. Scene 8 places all three devices in one horizontal row on desktop and mobile, with clear gaps and a shared world scale so the 312.6 mm laptop, 249.7 mm tablet, and 78 mm phone remain physically proportional. The stills contain no embedded closing copy.

The generated set depicts a 2025 14-inch MacBook Pro M5, 11-inch iPad Pro M5, and iPhone 17 Pro Max. Enclosure shells use outward-facing geometry. The iPad body and screen use concentric 15.05 mm and 6.6 mm corner contours across the roughly 8.5 mm bezel. Aluminum body materials use metallic 1.0 with no clearcoat, leaving the dark-strip studio environment to describe their shape. The proportions, black glass, camera details, controls, and ports follow the reference study in `device-references.md`. The geometry is original, with no third-party models or Apple marks.

## Satin silver finish

The #77 fixture uses satin silver aluminum, a darker satin laptop lid shell, dark
screen-facing bands on the tablet and phone bodies, and a matching satin trackpad.
The existing material names remain stable; the lid shell and front bands have their
own materials. These are
**linear RGB** values in both the saved Blender Principled materials and exported glTF PBR
materials; do not apply an sRGB conversion.

| Material | Base color | Metallic | Roughness |
|---|---|---|---|
| SpaceBlackAluminum | 0.65, 0.66, 0.68 | 1.0 | 0.45 |
| SatinLidAluminum (laptop lid shell only) | 0.12, 0.13, 0.14 | 1.0 | 0.60 |
| SatinFrontBand (tablet and phone front triangles only) | 0.12, 0.13, 0.14 | 1.0 | 0.60 |
| MachinedSpaceBlackEdge | 0.72, 0.73, 0.75 | 1.0 | 0.20 |
| TrackpadSpaceBlack | 0.50, 0.51, 0.53 | 1.0 | 0.32 |
| DeepBlueAluminum | 0.65, 0.66, 0.68 | 1.0 | 0.45 |
| DeepBlueMachinedEdge | 0.72, 0.73, 0.75 | 1.0 | 0.20 |
| DeepBlueCeramicShield | 0.70, 0.71, 0.72 | 0.15 | 0.38 |
| CameraRing | 0.70, 0.71, 0.73 | 1.0 | 0.16 |

The five satin materials have coat weight 0. The front band uses the runtime's
`normal.z > 0.5` triangle split after Blender's Z-up to glTF Y-up conversion;
the bodies' sides and backs keep their existing aluminum. Other material settings
retain their existing values. `tablet_body` and `phone_body` each export two material
primitives. GLTFLoader keeps each body name on a group: `tablet_body_mesh` and
`phone_body_mesh` are the silver chassis; `tablet_body_mesh_1` and `phone_body_mesh_1`
use `SatinFrontBand`. That material deliberately has no `Aluminum` suffix, so the
runtime's generic chassis dressing preserves the native dark finish.
The render rig reconstructs the film's eight emissive studio cards as a generated linear
2048×1024 environment, so rough metal reflects the whole studio rather than only direct
lights. It also uses the film's key, fill and edge directions and strengths; only the key
casts shadows. The black camera background, AgX output transform and document compositions
stay unchanged. Blender and the film's ACES/PMREM renderer are visually matched, not
pixel-identical. The environment is generated in memory during rendering and is not an
additional shipped asset. Runtime finish overrides are redundant once these GLBs are
integrated; removing them belongs to the film implementation.
