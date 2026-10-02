# Temporary #336 notification wall preview

`/lab/wall-646fe5bc6ee6/` is an unlisted copy of the full landing story with
the proposed hero. The film and document layouts can be reviewed together.
The lab route serves static HTML, so its GitHub link is filled in the page and
its activity slot is empty. It has noindex/nofollow metadata and is linked
from neither the home page nor the existing notifications lab.

The hero implementation lives in this directory while the default home hero
continues to use `src/hero/`. `preview.css` takes the established landing,
film, and hero styles, then adds the wall treatment. `build.mjs` runs a
separate Vite build after Astro so preview code does not split or rewrite the
home page's bundle. The generated-page test pins the home page, notifications
lab, and their assets to the bytes emitted by main.

`hero336-boot.js` replays the entrance on every load without changing the
home page's played marker. Reduced motion, `?hero=0`, and links to later acts
keep their usual behavior. This is Step 1 only; acceptance would require a
separate decision to replace the default hero.

The field starts at staggered ages so it is full immediately. A notification
slides 24px into its own place in 0.32s, holds for about two seconds, and
fades in 0.4s. Positions never scroll. Alternating row offsets pack the
cards across the visible hero; two intersecting masks soften all four edges.
Three final requests arrive at 2.18 / 2.36 / 2.54s, become mint, then fly
into their existing laptop rows. The full entrance settles at 5.05s.

`web/landing-notifications-check.mjs` checks this page at 390×667,
390×844, 768×1024, 1280×900, 1920×1080 and 2560×1440, including both
film and document layouts where available. It measures rendered edge alpha,
occupancy, local entry, still holds, replacement, and the mint row landings.
