# Dev icons

`favicon-dev.svg` is `public/favicon.svg` with only the two fill colors swapped.
The PNGs are rendered from that SVG using librsvg (`rsvg-convert`), without
changing the geometry. From `spa/`, regenerate the PNGs with:

```sh
rsvg-convert -w 32 -h 32 channel-icons/favicon-dev.svg -o channel-icons/favicon-dev.png
rsvg-convert -w 180 -h 180 channel-icons/favicon-dev.svg -o channel-icons/apple-touch-icon-dev.png
rsvg-convert -w 192 -h 192 channel-icons/favicon-dev.svg -o channel-icons/icon-dev-192.png
rsvg-convert -w 512 -h 512 channel-icons/favicon-dev.svg -o channel-icons/icon-dev-512.png
```

Vite emits these assets and `manifest-dev.webmanifest` only when
`VITE_BUILD_CHANNEL=dev`. That build also serves dev bytes at the original
manifest and icon URLs for existing PWA installs and push notifications.
The app image copies the dev favicons to the
landing URLs when assembled with `APP_CONFIG=app.channel-dev.yaml`, covering
the homepage, public pages and sign-in pages without changing their markup.
