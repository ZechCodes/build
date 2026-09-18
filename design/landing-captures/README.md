# Landing screen captures

This isolated capture fixture uses the current SPA visual tokens, bundled Inter files, and approved harness artwork without changing the product client. Its data is an explicit demonstration fixture for the landing narrative; controls do not call a repository or merge changes.

Run Vite from the repository root with `spa/node_modules/.bin/vite . --port 4178`, then run `node design/landing-captures/capture.mjs`. Set `CAPTURE_ORIGIN` when Vite uses another origin and `CHROMIUM_PATH` when Playwright's managed browser is not installed. The script resolves Playwright from `web/package.json` and writes 2× lossless PNG masters to `design/landing-captures/masters/`.

Create the 1× screen maps with ImageMagick:

```sh
for image in design/landing-captures/masters/*.png; do
  name=$(basename "${image%.png}")
  magick "$image" -resize 50% -quality 88 "skriftapp/buildapp/landing/assets/screens/${name}.webp"
done
```

The fixture imports the current `inboxRowHtml`, `threadHtml`, `agentSurfacesRender`, `gitToolbarHtml`, `diffRowsHtml`, and directory-rail renderers. Its surrounding frame is a deterministic reconstruction for marketing capture, not a claim that the demonstration data came from a live host.

The exact workspace, state order, crops, timers, and text-safe areas are recorded in `screen-manifest.json`.
