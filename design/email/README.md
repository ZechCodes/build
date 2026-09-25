# Email images

Every Build email links to PNGs in `skriftapp/buildapp/landing/email/`, which the app serves at
`/landing/email/<file>` with a one-year cache (`RootController.landing_asset`). The template
(`skriftapp/buildapp/email_template.py`) builds each URL from the deployment's public base URL and
declares each image's display size.

| File | Shown at | Source |
| --- | --- | --- |
| `brand-mark.png` | 80 × 32 (1x, `srcset`) | `skriftapp/buildapp/landing/brand-mark.svg` |
| `brand-mark@2x.png` | 80 × 32 (the `src`) | the same SVG |
| `phone-tablet.png` | 560 × 378, the layout's full width | `tablet-screen.png` and `phone-screen.png` on the landing's iPad and phone |

The images are cached for a year under stable names. A replacement image may not show on a
client that has already fetched the old one until that year runs out. If the change must reach
everyone straight away, give the file a new name and update `email_template.py`.

## Brand mark

The mark is 32 px tall and sits at the left of a transparent 80 × 32 box. A client that
blocks images shows the alt text "Build" in that box, and at 27 px wide the mark alone
would cut it to "Bui". From the repository root:

```sh
for scale in 1 2; do
  suffix=$([ "$scale" = 2 ] && echo "@2x")
  rsvg-convert -h $((32 * scale)) skriftapp/buildapp/landing/brand-mark.svg \
    | magick png:- -background none -gravity west -extent $((80 * scale))x$((32 * scale)) \
      -strip PNG32:skriftapp/buildapp/landing/email/brand-mark$suffix.png
done
```

## Phone and tablet

The invite's hero is the landing's iPad and phone, tipped away from each other. Both screens
come from the landing capture fixture (`design/landing-captures/`), which draws the SPA's own
markup with its shipped stylesheets, dark theme, with demonstration data only and the host
named `dev-mbp` (see `design/landing-captures/README.md`):

- `tablet-screen.png`: the Changes view of the demonstration workspace `archive-search`
  (`ui14-git`) at the iPad profile, 2420 × 1668.
- `phone-screen.png`: the inbox, opened from its toggle as the app opens it at phone width,
  over the Implement conversation (`ui03`, resumed) at the iPhone profile, 1320 × 2868.

1. Capture the screens. Serve the repository root with Vite, then capture both. The script
   writes only these two files, so the landing's masters and WebP files stay as they are:

   ```sh
   spa/node_modules/.bin/vite . --port 4178
   CAPTURE_ORIGIN=http://localhost:4178 CHROMIUM_PATH=/usr/bin/chromium \
     node design/email/capture_screens.mjs
   ```

2. Render them on the landing's devices. This opens `build-devices.blend` without saving it,
   and uses the landing generator's studio lights, camera fit and screen mapping. The tablet
   stands left and the phone in front of its right edge; each turns 20° the opposite way
   about the vertical axis, so the edges where they meet come forward. It takes about 5 s:

   ```sh
   blender --background design/landing/build-devices.blend \
     --python design/email/render_pair.py -- \
     --tablet-screen design/email/tablet-screen.png \
     --phone-screen design/email/phone-screen.png --out /tmp/pair-render.png
   ```

3. Compose the email image. This crops the render to the hardware, lays it on the email's
   `#030604` background, scales it to 1200 px wide and remaps it to a dithered 256-colour
   palette (about 160 KB; the budget is 250 KB):

   ```sh
   skriftapp/.venv/bin/python design/email/compose_hero.py \
     /tmp/pair-render.png skriftapp/buildapp/landing/email/phone-tablet.png
   ```

If the composed height changes, update `PHONE_TABLET_IMAGE` in `email_template.py`.
`test_email_assets.py` fails when a file's aspect ratio no longer matches its declared size.
