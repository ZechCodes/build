# Email images

Every Build email links to PNGs in `skriftapp/buildapp/landing/email/`, which the app serves at
`/landing/email/<file>` with a one-year cache (`RootController.landing_asset`). The template
(`skriftapp/buildapp/email_template.py`) builds each URL from the deployment's public base URL and
declares each image's display size.

| File | Shown at | Source |
| --- | --- | --- |
| `brand-mark.png` | 80 × 32 (1x, `srcset`) | `skriftapp/buildapp/landing/brand-mark.svg` |
| `brand-mark@2x.png` | 80 × 32 (the `src`) | the same SVG |
| `laptop.png` | 560 × 308, the layout's full width | `laptop-screen.png` on the landing's MacBook |

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

## Laptop

`laptop-screen.png` is the landing capture fixture's `ui14-git` scene: the Changes view of the
demonstration workspace `archive-search`, at the MacBook profile (3024 × 1964, dark theme). The
fixture draws the SPA's own markup with its shipped stylesheets, so this is the app as it currently
renders. It uses only demonstration data, with the host named `dev-mbp` (see
`design/landing-captures/README.md`).

1. Capture the screen. Serve the repository root with Vite, then capture only this scene:

   ```sh
   spa/node_modules/.bin/vite . --port 4178
   CAPTURE_ORIGIN=http://localhost:4178 CAPTURE_SCENES=ui14-git CAPTURE_PROFILES=macbook \
     CHROMIUM_PATH=/usr/bin/chromium node design/landing-captures/capture.mjs
   cp design/landing-captures/masters/ui14-git-macbook.png design/email/laptop-screen.png
   git checkout -- design/landing-captures skriftapp/buildapp/landing/assets/screens
   ```

   The capture script also rewrites the landing's master, WebP and results file. The checkout
   puts them back, so that an email change does not also change the landing.

2. Render it on the landing's laptop. This opens `build-devices.blend` without saving it, and
   uses the landing generator's studio lights, hero camera and screen mapping. It takes about 5 s:

   ```sh
   blender --background design/landing/build-devices.blend \
     --python design/email/render_laptop.py -- \
     --screen design/email/laptop-screen.png --out /tmp/laptop-render.png
   ```

3. Compose the email image. This crops the render to the hardware, lays it on the email's
   `#030604` background, scales it to 1200 px wide and reduces it to a 256-colour palette
   (about 170 KB; the budget is 250 KB):

   ```sh
   skriftapp/.venv/bin/python design/email/compose_laptop.py \
     /tmp/laptop-render.png skriftapp/buildapp/landing/email/laptop.png
   ```

If the composed height changes, update `LAPTOP_IMAGE` in `email_template.py`.
`test_email_assets.py` fails when a file's aspect ratio no longer matches its declared size.
